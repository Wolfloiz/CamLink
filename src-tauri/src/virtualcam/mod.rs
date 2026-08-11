//! Abstração de webcam virtual: trait `VirtualCameraBackend`
//! (create/destroy/feed_frame/set_standby). Único módulo com código
//! platform-specific (Princípio IV); invariante: 1 fonte ↔ 1 device.

#[cfg(target_os = "windows")]
pub mod dshow;
// Cross-platform de propósito: a lógica de pacing do filtro DirectShow é
// pura para ser testável no Linux (onde dshow.rs nem compila).
pub mod pacing;
#[cfg(target_os = "linux")]
pub mod v4l2;

use std::collections::HashMap;

use uuid::Uuid;

use crate::error::AppError;
use crate::model::{AndroidDevice, VirtualCamera, VirtualCameraState};

/// Limite prático de fontes simultâneas da v1 (spec.md, notas; FR-021;
/// plan.md "Scale/Scope").
pub const MAX_CONCURRENT_SOURCES: usize = 4;

/// Gate de capacidade chamado por `start_stream`/`start_rtsp` (lib.rs) antes
/// de criar qualquer recurso (vcam/processo) para a nova fonte — nunca deixa
/// uma 5ª fonte nascer parcialmente alocada.
pub fn check_capacity(active: usize) -> Result<(), AppError> {
    if active >= MAX_CONCURRENT_SOURCES {
        Err(AppError::new(
            "max_sources_reached",
            format!("Limite de {MAX_CONCURRENT_SOURCES} fontes simultâneas atingido"),
        ))
    } else {
        Ok(())
    }
}

/// Comprimento máximo seguro de um label de device virtual (T065c): o
/// `card_label` do v4l2loopback é um `char[32]` do lado do kernel (31 chars
/// úteis + terminador), e o filtro DirectShow tem uma restrição do mesmo
/// tipo. Sem cortar aqui, um nome digitado livremente (fonte RTSP) ou um
/// discriminador longo (modelo Android + sufixo) falha ou trunca de forma
/// feia no OBS em vez de na origem.
pub const MAX_LABEL_LEN: usize = 31;

/// Normaliza texto pra virar (parte de) um label de device: colapsa
/// sequências de espaço em um só, tira espaço nas pontas e corta em
/// `max_len` caracteres — por `char`, nunca por byte, então não quebra
/// UTF-8 ao meio.
pub fn sanitize_label(raw: &str, max_len: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_len)
        .collect()
}

/// Label por-fonte usado na criação do device virtual: cada fonte
/// concorrente (serial do Android, id da fonte RTSP) precisa de um label
/// distinto, senão `find_reusable_device` (v4l2.rs) reaproveita/rouba o
/// device de uma sessão irmã ainda viva. O reuso continua estável ENTRE
/// restarts da MESMA fonte lógica porque o discriminador (serial/id) não
/// muda entre chamadas para a mesma fonte. Sempre sanitizado/cortado em
/// `MAX_LABEL_LEN` (T065c) — discriminadores digitados livremente (nome de
/// fonte RTSP) não podem estourar o limite do device virtual.
pub fn vcam_label(base: &str, discriminator: &str) -> String {
    sanitize_label(&format!("{base} ({discriminator})"), MAX_LABEL_LEN)
}

/// Últimos 4 chars do serial — sufixo curto de desambiguação reaproveitado
/// tanto pelo nome do modelo quanto pelo apelido (T065e).
fn serial_suffix(serial: &str) -> String {
    let chars: Vec<char> = serial.chars().collect();
    let start = chars.len().saturating_sub(4);
    chars[start..].iter().collect()
}

/// Discriminador amigável de fonte Android (T065c/T065e): prioriza o
/// apelido definido pelo usuário (`nickname`, ex. "Câmera lateral" — o
/// `model` do adb costuma ser só o nome de código comercial, tipo
/// "SM-S921B", não o nome de marketing "Galaxy S24"; o app não tem como
/// saber esse mapeamento, então deixar o usuário nomear é o caminho). Sem
/// apelido, cai pro modelo do aparelho (`AndroidDevice.model`, cacheado em
/// `AppState.devices` desde a última descoberta via `adb devices -l`) em
/// vez do serial cru. Em ambos os casos leva um sufixo do próprio serial
/// pra continuar único mesmo com 2 aparelhos do MESMO modelo/apelido
/// plugados ao mesmo tempo (ex.: `"Câmera lateral (2ABC)"`,
/// `"SM-N970F (2ABC)"`) — o discriminador também é a chave de unicidade do
/// device virtual (`find_reusable_device`). Se o device ainda não estiver
/// na lista cacheada (corrida rara entre descoberta e start) ou o modelo
/// vier vazio, cai pro serial puro — nunca falha, só fica menos amigável.
pub fn android_label_discriminator(
    devices: &[AndroidDevice],
    serial: &str,
    nickname: Option<&str>,
) -> String {
    if let Some(nickname) = nickname.map(str::trim).filter(|n| !n.is_empty()) {
        return format!("{nickname} ({})", serial_suffix(serial));
    }
    match devices.iter().find(|d| d.serial == serial) {
        Some(d) if !d.model.trim().is_empty() => {
            format!("{} ({})", d.model.trim(), serial_suffix(serial))
        }
        _ => serial.to_string(),
    }
}

/// Discriminador amigável de fonte RTSP (T065c): usa o nome que o próprio
/// usuário deu à fonte (`RtspSource.name`, ex. "Câmera do portão") em vez
/// do `Uuid` cru. Sempre leva um sufixo curto do `id` — mesmo que o nome
/// seja único hoje, nada impede o usuário de cadastrar duas fontes com o
/// mesmo nome, e o discriminador também é a chave de unicidade do device
/// virtual (`find_reusable_device`); sem o sufixo, a 2ª fonte roubaria o
/// device da 1ª (o mesmo bug corrigido no T062 pra Android). Nome vazio
/// (não deveria acontecer — `RtspPanel.svelte` exige preenchido — mas sem
/// custo cobrir) cai pro `id` puro.
pub fn rtsp_label_discriminator(name: &str, id: &Uuid) -> String {
    let name = name.trim();
    let suffix: String = id.simple().to_string().chars().take(4).collect();
    if name.is_empty() {
        id.to_string()
    } else {
        format!("{name} #{suffix}")
    }
}

/// Falha do backend de câmera virtual.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VcamError {
    #[error("câmera virtual não encontrada: {0}")]
    NotFound(Uuid),
    #[error("frame inválido: {0}")]
    InvalidFrame(String),
    #[error("falha do backend de câmera virtual: {0}")]
    Backend(String),
}

/// Backend de câmera virtual por plataforma (Linux: v4l2loopback; Windows:
/// akvirtualcamera). Object-safe: os managers selecionam a implementação em
/// runtime via `Box<dyn VirtualCameraBackend>`.
///
/// Contrato: o device nasce em `Standby` exibindo a imagem de espera
/// (FR-006) e passa a `Live` no primeiro `feed_frame`; cada fonte ativa tem
/// exatamente um device (FR-021) e operações sobre device destruído retornam
/// erro, nunca pânico.
pub trait VirtualCameraBackend {
    /// Cria um device virtual com o `label` visível nos apps consumidores.
    fn create(
        &mut self,
        label: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<VirtualCamera, VcamError>;

    /// Cria um device SEMPRE novo, sem tentar reaproveitar um existente com
    /// o mesmo `label` — usado quando a resolução de saída pode ter mudado
    /// (giro 90°/270° ou troca de câmera). Reaproveitar arriscaria herdar um
    /// device cujo formato ficou travado por um consumidor (Chrome/Meet)
    /// ainda aberto nele: confirmado em bancada 2026-07-27 — um writer novo
    /// com resolução diferente é silenciosamente ignorado pelo
    /// v4l2loopback enquanto o leitor antigo não fecha o device, entregando
    /// frames com a geometria errada (câmera some do Meet e não volta).
    /// Default: delega a `create` (Windows já recria o filtro inteiro do
    /// zero a cada restart, então reaproveitar nunca se aplicou ali).
    fn create_fresh(
        &mut self,
        label: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<VirtualCamera, VcamError> {
        self.create(label, resolution, fps)
    }

    /// Empurra um frame RGBA para o device; transiciona para `Live`.
    fn feed_frame(&mut self, id: &Uuid, frame: &[u8]) -> Result<(), VcamError>;

    /// Exibe a imagem de espera com a mensagem de estado (FR-006);
    /// transiciona para `Standby`.
    fn set_standby(&mut self, id: &Uuid, message: &str) -> Result<(), VcamError>;

    /// Remove o device, liberando os apps consumidores sem congelá-los.
    fn destroy(&mut self, id: &Uuid) -> Result<(), VcamError>;

    fn camera(&self, id: &Uuid) -> Option<&VirtualCamera>;

    fn cameras(&self) -> Vec<&VirtualCamera>;

    /// Colapsa devices duplicados (mesmo label) deixados por sessões
    /// anteriores que não puderam ser removidos na hora (estavam ocupados
    /// por um consumidor) de volta a no máximo um por label. Chamado uma
    /// vez na construção do backend — nesse momento nenhuma sessão do
    /// CamLink está rodando ainda, então é o ponto mais seguro/eficaz pra
    /// arrumar o que ficou pra trás sem depender de outro restart
    /// acontecer (achado em bancada 2026-07-27: sem isso, o Meet/OBS podia
    /// acumular vários "CamLink Android" na lista de devices). Default:
    /// no-op (Windows não tem esse tipo de duplicata).
    fn cleanup_stale(&mut self) {}

    /// Remove definitivamente todos os devices desta execução do app —
    /// diferente de `destroy`, que mantém o device propositalmente para
    /// reuso entre sessões (troca de câmera, restart) enquanto o app
    /// continua rodando. Chamado só nos hooks de encerramento do processo
    /// inteiro (fechar a janela, Ctrl+C/SIGTERM): sem isso, o device v4l2
    /// ficava registrado para sempre depois de fechar o app — o `ffmpeg`
    /// que o alimentava morria (stdin fecha com o processo pai), mas o
    /// device continuava listado no OBS/Chrome, agora sem ninguém
    /// escrevendo nele ("câmera inacessível", achado em bancada
    /// 2026-08-11). Default: no-op (Windows recria o filtro DirectShow do
    /// zero a cada `start_stream`; nada fica pendurado ao fechar).
    fn purge_all(&mut self) {}
}

/// Gera a imagem de espera (FR-006): fundo escuro com o glifo de câmera do
/// CamLink centralizado e uma barra proporcional à mensagem de estado.
/// Buffer RGBA de `width × height × 4`, determinística para os mesmos
/// argumentos.
///
/// A renderização de texto real da mensagem entra junto com os backends de
/// plataforma; a assinatura já recebe `message` para manter o contrato.
pub fn standby_frame(resolution: (u32, u32), message: &str) -> Vec<u8> {
    let (width, height) = resolution;
    let (w, h) = (i64::from(width), i64::from(height));
    let (cx, cy) = (w / 2, h / 2);

    // Geometria do glifo: corpo retangular de câmera com lente circular.
    let body_w = w / 4;
    let body_h = h / 6;
    let lens_r = (body_h * 2) / 5;
    // Barra sob o glifo, com largura proporcional à mensagem (placeholder do
    // texto renderizado).
    let bar_y = cy + body_h;
    let bar_half_w = ((message.chars().count() as i64) * w / 160).clamp(w / 20, (w * 2) / 5);
    let bar_half_h = (h / 180).max(1);

    const BG: [u8; 4] = [16, 18, 26, 255];
    const BODY: [u8; 4] = [58, 64, 86, 255];
    const ACCENT: [u8; 4] = [120, 180, 255, 255];

    let mut buf = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x - cx, y - cy);
            let pixel = if dx * dx + dy * dy <= lens_r * lens_r {
                ACCENT
            } else if dx.abs() <= body_w / 2 && dy.abs() <= body_h / 2 {
                BODY
            } else if (y - bar_y).abs() <= bar_half_h && dx.abs() <= bar_half_w {
                ACCENT
            } else {
                BG
            };
            buf.extend_from_slice(&pixel);
        }
    }
    buf
}

struct MockCamera {
    camera: VirtualCamera,
    frames_fed: usize,
    last_standby_message: Option<String>,
}

/// Backend em memória para testes: registra frames e mensagens de espera
/// sem tocar em device real.
#[derive(Default)]
pub struct MockBackend {
    cameras: HashMap<Uuid, MockCamera>,
    next_index: u64,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Quantos frames já foram empurrados para o device (0 se não existe).
    pub fn frames_fed(&self, id: &Uuid) -> usize {
        self.cameras.get(id).map_or(0, |c| c.frames_fed)
    }

    /// Última mensagem de espera exibida no device.
    pub fn last_standby_message(&self, id: &Uuid) -> Option<String> {
        self.cameras
            .get(id)
            .and_then(|c| c.last_standby_message.clone())
    }

    fn entry(&mut self, id: &Uuid) -> Result<&mut MockCamera, VcamError> {
        self.cameras.get_mut(id).ok_or(VcamError::NotFound(*id))
    }
}

impl VirtualCameraBackend for MockBackend {
    fn create(
        &mut self,
        label: &str,
        _resolution: (u32, u32),
        _fps: u32,
    ) -> Result<VirtualCamera, VcamError> {
        let camera = VirtualCamera {
            id: Uuid::new_v4(),
            label: label.to_string(),
            backend_path: format!("/dev/mock-video{}", self.next_index),
            state: VirtualCameraState::Standby,
        };
        self.next_index += 1;
        self.cameras.insert(
            camera.id,
            MockCamera {
                camera: camera.clone(),
                frames_fed: 0,
                last_standby_message: None,
            },
        );
        tracing::debug!(id = %camera.id, path = %camera.backend_path, "mock vcam criada");
        Ok(camera)
    }

    fn feed_frame(&mut self, id: &Uuid, frame: &[u8]) -> Result<(), VcamError> {
        if frame.is_empty() {
            return Err(VcamError::InvalidFrame("frame vazio".into()));
        }
        let entry = self.entry(id)?;
        entry.frames_fed += 1;
        entry.camera.state = VirtualCameraState::Live;
        Ok(())
    }

    fn set_standby(&mut self, id: &Uuid, message: &str) -> Result<(), VcamError> {
        let entry = self.entry(id)?;
        entry.camera.state = VirtualCameraState::Standby;
        entry.last_standby_message = Some(message.to_string());
        Ok(())
    }

    fn destroy(&mut self, id: &Uuid) -> Result<(), VcamError> {
        self.cameras
            .remove(id)
            .map(|_| ())
            .ok_or(VcamError::NotFound(*id))
    }

    fn camera(&self, id: &Uuid) -> Option<&VirtualCamera> {
        self.cameras.get(id).map(|c| &c.camera)
    }

    fn cameras(&self) -> Vec<&VirtualCamera> {
        self.cameras.values().map(|c| &c.camera).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AuthState;

    fn device(serial: &str, model: &str) -> AndroidDevice {
        AndroidDevice {
            serial: serial.to_string(),
            model: model.to_string(),
            auth_state: AuthState::Authorized,
            compatible: true,
            incompat_reason: None,
            capabilities: None,
        }
    }

    // -----------------------------------------------------------------
    // sanitize_label
    // -----------------------------------------------------------------

    #[test]
    fn sanitize_label_trims_and_collapses_internal_whitespace() {
        assert_eq!(
            sanitize_label("  Câmera   do   portão  ", 31),
            "Câmera do portão"
        );
    }

    #[test]
    fn sanitize_label_truncates_by_char_not_byte() {
        // "ã" ocupa 2 bytes em UTF-8 — cortar por byte quebraria o char.
        let raw = "ãããããããããã"; // 12 chars
        let sanitized = sanitize_label(raw, 5);
        assert_eq!(sanitized.chars().count(), 5);
        assert_eq!(sanitized, "ããããã");
    }

    #[test]
    fn sanitize_label_leaves_short_text_untouched() {
        assert_eq!(sanitize_label("Câmera do portão", 31), "Câmera do portão");
    }

    // -----------------------------------------------------------------
    // vcam_label — nunca estoura MAX_LABEL_LEN (T065c), mesmo com um
    // discriminador digitado livremente (nome de fonte RTSP).
    // -----------------------------------------------------------------

    #[test]
    fn vcam_label_never_exceeds_max_label_len() {
        let long_name = "Câmera de segurança da entrada principal do galpão 2";
        let label = vcam_label("CamLink IP", long_name);
        assert!(label.chars().count() <= MAX_LABEL_LEN);
    }

    #[test]
    fn vcam_label_short_inputs_unaffected_by_truncation() {
        assert_eq!(
            vcam_label("CamLink Android", "R58M12ABCDE"),
            "CamLink Android (R58M12ABCDE)"
        );
    }

    // -----------------------------------------------------------------
    // android_label_discriminator
    // -----------------------------------------------------------------

    #[test]
    fn android_label_discriminator_uses_model_and_serial_suffix() {
        let devices = vec![device("R58M12ABCDE", "SM-N970F")];
        assert_eq!(
            android_label_discriminator(&devices, "R58M12ABCDE", None),
            "SM-N970F (BCDE)"
        );
    }

    #[test]
    fn android_label_discriminator_disambiguates_same_model_different_devices() {
        let devices = vec![
            device("R58M12ABCDE", "SM-N970F"),
            device("HT2ABC091234", "SM-N970F"),
        ];
        let a = android_label_discriminator(&devices, "R58M12ABCDE", None);
        let b = android_label_discriminator(&devices, "HT2ABC091234", None);
        assert_ne!(a, b);
        assert!(a.starts_with("SM-N970F ("));
        assert!(b.starts_with("SM-N970F ("));
    }

    #[test]
    fn android_label_discriminator_falls_back_to_serial_when_device_unknown() {
        let devices: Vec<AndroidDevice> = vec![];
        assert_eq!(
            android_label_discriminator(&devices, "R58M12ABCDE", None),
            "R58M12ABCDE"
        );
    }

    #[test]
    fn android_label_discriminator_falls_back_to_serial_when_model_is_blank() {
        let devices = vec![device("R58M12ABCDE", "  ")];
        assert_eq!(
            android_label_discriminator(&devices, "R58M12ABCDE", None),
            "R58M12ABCDE"
        );
    }

    #[test]
    fn android_label_discriminator_prefers_nickname_over_model() {
        let devices = vec![device("R58M12ABCDE", "SM-S921B")];
        assert_eq!(
            android_label_discriminator(&devices, "R58M12ABCDE", Some("Câmera lateral")),
            "Câmera lateral (BCDE)"
        );
    }

    #[test]
    fn android_label_discriminator_nickname_disambiguates_same_nickname_different_devices() {
        let devices = vec![
            device("R58M12ABCDE", "SM-S921B"),
            device("HT2ABC091234", "SM-S921B"),
        ];
        let a = android_label_discriminator(&devices, "R58M12ABCDE", Some("Câmera lateral"));
        let b = android_label_discriminator(&devices, "HT2ABC091234", Some("Câmera lateral"));
        assert_ne!(a, b);
    }

    #[test]
    fn android_label_discriminator_blank_nickname_falls_back_to_model() {
        let devices = vec![device("R58M12ABCDE", "SM-S921B")];
        assert_eq!(
            android_label_discriminator(&devices, "R58M12ABCDE", Some("   ")),
            "SM-S921B (BCDE)"
        );
    }

    // -----------------------------------------------------------------
    // rtsp_label_discriminator
    // -----------------------------------------------------------------

    #[test]
    fn rtsp_label_discriminator_uses_the_user_given_name() {
        let id = Uuid::new_v4();
        let discriminator = rtsp_label_discriminator("Câmera do portão", &id);
        assert!(discriminator.starts_with("Câmera do portão #"));
    }

    #[test]
    fn rtsp_label_discriminator_disambiguates_sources_with_the_same_name() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        assert_ne!(
            rtsp_label_discriminator("Câmera", &id_a),
            rtsp_label_discriminator("Câmera", &id_b)
        );
    }

    #[test]
    fn rtsp_label_discriminator_falls_back_to_id_when_name_is_blank() {
        let id = Uuid::new_v4();
        assert_eq!(rtsp_label_discriminator("   ", &id), id.to_string());
    }
}
