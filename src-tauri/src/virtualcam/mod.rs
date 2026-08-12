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

/// Prefixo comum a TODO label criado por este app. É a única ligação entre
/// "o que o app cria" e "o que o app tem direito de apagar" na limpeza de
/// devices órfãos (`v4l2::orphan_devices`), e também o que agrupa as fontes
/// do CamLink na lista do OBS. Coberto pelos testes
/// `vcam_label_always_starts_with_the_prefix` (aqui) e
/// `labels_created_by_this_app_are_recognized_as_its_own_orphans`
/// (`tests/v4l2_test.rs`), que fecham o ciclo cria→reconhece.
pub const LABEL_PREFIX: &str = "CamLink";

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

/// Label do device virtual: `"CamLink {name} {suffix}"` — o que o usuário vê
/// no seletor de câmera do OBS.
///
/// `suffix` é a CHAVE de unicidade: `find_reusable_device` (v4l2.rs) casa o
/// label inteiro, então duas fontes concorrentes com labels iguais roubam o
/// device uma da outra (bug do T062). Por isso ele **nunca** é truncado — só
/// o `name` é cortado pra caber em `MAX_LABEL_LEN`. A versão anterior
/// truncava a string composta inteira, o que podia comer o sufixo e ainda
/// deixava parêntese aberto (visto em bancada 2026-08-11:
/// `CamLink Android (SM_S921B (P11P` e `CamLink Android (camera para o `).
///
/// O orçamento sobrante pro nome é 18 chars com o sufixo típico de 4
/// (`31 - "CamLink" - 2 espaços - 4`). O reuso entre restarts da MESMA fonte
/// lógica continua estável porque nome e sufixo não mudam entre chamadas.
pub fn vcam_label(name: &str, suffix: &str) -> String {
    let suffix = sanitize_label(suffix, MAX_LABEL_LEN);
    // "CamLink" + espaço antes do nome + espaço antes do sufixo + sufixo.
    let fixed = LABEL_PREFIX.chars().count() + 2 + suffix.chars().count();
    let name = sanitize_label(name, MAX_LABEL_LEN.saturating_sub(fixed));
    let composed = if name.is_empty() {
        format!("{LABEL_PREFIX} {suffix}")
    } else {
        format!("{LABEL_PREFIX} {name} {suffix}")
    };
    // Rede de segurança pra um sufixo absurdamente longo (nenhum chamador
    // gera isso hoje — todos usam 4 chars).
    sanitize_label(&composed, MAX_LABEL_LEN)
}

/// Últimos 4 chars do serial — sufixo curto de desambiguação reaproveitado
/// tanto pelo nome do modelo quanto pelo apelido (T065e).
fn serial_suffix(serial: &str) -> String {
    let chars: Vec<char> = serial.chars().collect();
    let start = chars.len().saturating_sub(4);
    chars[start..].iter().collect()
}

/// `(nome, sufixo)` de uma fonte Android para `vcam_label` (T065c/T065e).
///
/// O nome prioriza o apelido dado pelo usuário (`nickname`, ex. "câmera
/// teto") — o `model` do adb costuma ser só o nome de código comercial
/// ("SM_S921B"), não o de marketing ("Galaxy S24"), e o app não tem como
/// saber esse mapeamento sozinho. Sem apelido, cai pro modelo
/// (`AndroidDevice.model`, cacheado em `AppState.devices` desde a última
/// descoberta via `adb devices -l`).
///
/// O sufixo são os 4 últimos chars do serial, o que mantém devices
/// distintos mesmo com 2 aparelhos do MESMO modelo/apelido plugados juntos
/// (ex.: `CamLink câmera teto P11P` vs `CamLink câmera teto 3D5B`).
pub fn android_label_parts(
    devices: &[AndroidDevice],
    serial: &str,
    nickname: Option<&str>,
) -> (String, String) {
    let suffix = serial_suffix(serial);
    if let Some(nickname) = nickname.map(str::trim).filter(|n| !n.is_empty()) {
        return (nickname.to_string(), suffix);
    }
    match devices.iter().find(|d| d.serial == serial) {
        Some(d) if !d.model.trim().is_empty() => (d.model.trim().to_string(), suffix),
        // Sem nome nenhum pra mostrar: o serial inteiro vira o sufixo, que
        // continua garantindo unicidade (só fica menos amigável).
        _ => (String::new(), serial.to_string()),
    }
}

/// `(nome, sufixo)` de uma fonte RTSP para `vcam_label` (T065c).
///
/// O nome é o que o próprio usuário deu à fonte (`RtspSource.name`, ex.
/// "câmera do portão"). O sufixo vem do `id` e é obrigatório mesmo com nome
/// único: nada impede cadastrar duas fontes com o mesmo nome, e o label
/// inteiro é a chave de unicidade do device (`find_reusable_device`) — sem
/// o sufixo a 2ª fonte roubaria o device da 1ª (bug do T062). Nome vazio
/// (`RtspPanel.svelte` exige preenchido, mas cobrir é de graça) resulta em
/// `CamLink {sufixo}`, ainda único.
pub fn rtsp_label_parts(name: &str, id: &Uuid) -> (String, String) {
    let suffix: String = id.simple().to_string().chars().take(4).collect();
    (name.trim().to_string(), suffix)
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
    // vcam_label — cabe em MAX_LABEL_LEN sem NUNCA comer o sufixo de
    // unicidade nem deixar pontuação pela metade (achado em bancada
    // 2026-08-11: `CamLink Android (SM_S921B (P11P`).
    // -----------------------------------------------------------------

    #[test]
    fn vcam_label_never_exceeds_max_label_len() {
        let long_name = "Câmera de segurança da entrada principal do galpão 2";
        let label = vcam_label(long_name, "a1b2");
        assert!(label.chars().count() <= MAX_LABEL_LEN, "label: {label:?}");
    }

    /// O sufixo é a chave de unicidade — se o truncamento o comesse, duas
    /// fontes com nome longo e parecido colidiriam no mesmo device.
    #[test]
    fn vcam_label_keeps_the_whole_suffix_even_when_the_name_is_truncated() {
        let long_name = "Câmera de segurança da entrada principal do galpão 2";
        let a = vcam_label(long_name, "a1b2");
        let b = vcam_label(long_name, "c3d4");
        assert!(a.ends_with("a1b2"), "sufixo perdido em {a:?}");
        assert!(b.ends_with("c3d4"), "sufixo perdido em {b:?}");
        assert_ne!(
            a, b,
            "nomes longos iguais não podem colapsar no mesmo label"
        );
    }

    #[test]
    fn vcam_label_short_inputs_are_untouched() {
        assert_eq!(
            vcam_label("camera teto", "P11P"),
            "CamLink camera teto P11P"
        );
        assert_eq!(vcam_label("SM_S921B", "P11P"), "CamLink SM_S921B P11P");
    }

    #[test]
    fn vcam_label_without_a_name_is_still_unique() {
        assert_eq!(vcam_label("", "R58M12ABCDE"), "CamLink R58M12ABCDE");
    }

    /// Todo label criado aqui precisa casar com o prefixo, senão o device
    /// vira fantasma permanente (`v4l2::orphan_devices` nunca o limpa).
    #[test]
    fn vcam_label_always_starts_with_the_prefix() {
        for (name, suffix) in [
            ("camera teto", "P11P"),
            ("", "a1b2"),
            ("nome absurdamente longo que estoura o limite", "c3d4"),
        ] {
            assert!(vcam_label(name, suffix).starts_with(LABEL_PREFIX));
        }
    }

    // -----------------------------------------------------------------
    // android_label_parts
    // -----------------------------------------------------------------

    #[test]
    fn android_parts_use_model_and_serial_suffix() {
        let devices = vec![device("R58M12ABCDE", "SM-N970F")];
        let (name, suffix) = android_label_parts(&devices, "R58M12ABCDE", None);
        assert_eq!((name.as_str(), suffix.as_str()), ("SM-N970F", "BCDE"));
    }

    #[test]
    fn android_parts_disambiguate_same_model_different_devices() {
        let devices = vec![
            device("R58M12ABCDE", "SM-N970F"),
            device("HT2ABC091234", "SM-N970F"),
        ];
        let a = android_label_parts(&devices, "R58M12ABCDE", None);
        let b = android_label_parts(&devices, "HT2ABC091234", None);
        assert_eq!(a.0, b.0, "mesmo modelo, mesmo nome");
        assert_ne!(a.1, b.1, "sufixos precisam diferir");
        assert_ne!(vcam_label(&a.0, &a.1), vcam_label(&b.0, &b.1));
    }

    #[test]
    fn android_parts_fall_back_to_serial_when_device_unknown() {
        let devices: Vec<AndroidDevice> = vec![];
        let (name, suffix) = android_label_parts(&devices, "R58M12ABCDE", None);
        assert!(name.is_empty());
        assert_eq!(suffix, "R58M12ABCDE");
    }

    #[test]
    fn android_parts_fall_back_to_serial_when_model_is_blank() {
        let devices = vec![device("R58M12ABCDE", "  ")];
        let (name, suffix) = android_label_parts(&devices, "R58M12ABCDE", None);
        assert!(name.is_empty());
        assert_eq!(suffix, "R58M12ABCDE");
    }

    #[test]
    fn android_parts_prefer_nickname_over_model() {
        let devices = vec![device("R58M12ABCDE", "SM-S921B")];
        let (name, suffix) = android_label_parts(&devices, "R58M12ABCDE", Some("Câmera lateral"));
        assert_eq!((name.as_str(), suffix.as_str()), ("Câmera lateral", "BCDE"));
    }

    #[test]
    fn android_parts_nickname_disambiguates_same_nickname_different_devices() {
        let devices = vec![
            device("R58M12ABCDE", "SM-S921B"),
            device("HT2ABC091234", "SM-S921B"),
        ];
        let a = android_label_parts(&devices, "R58M12ABCDE", Some("Câmera lateral"));
        let b = android_label_parts(&devices, "HT2ABC091234", Some("Câmera lateral"));
        assert_ne!(vcam_label(&a.0, &a.1), vcam_label(&b.0, &b.1));
    }

    #[test]
    fn android_parts_blank_nickname_falls_back_to_model() {
        let devices = vec![device("R58M12ABCDE", "SM-S921B")];
        let (name, suffix) = android_label_parts(&devices, "R58M12ABCDE", Some("   "));
        assert_eq!((name.as_str(), suffix.as_str()), ("SM-S921B", "BCDE"));
    }

    // -----------------------------------------------------------------
    // rtsp_label_parts
    // -----------------------------------------------------------------

    #[test]
    fn rtsp_parts_use_the_user_given_name() {
        let id = Uuid::new_v4();
        let (name, suffix) = rtsp_label_parts("Câmera do portão", &id);
        assert_eq!(name, "Câmera do portão");
        assert_eq!(suffix.chars().count(), 4);
    }

    #[test]
    fn rtsp_parts_disambiguate_sources_with_the_same_name() {
        let (id_a, id_b) = (Uuid::new_v4(), Uuid::new_v4());
        let a = rtsp_label_parts("Câmera", &id_a);
        let b = rtsp_label_parts("Câmera", &id_b);
        assert_ne!(vcam_label(&a.0, &a.1), vcam_label(&b.0, &b.1));
    }

    #[test]
    fn rtsp_parts_with_blank_name_still_produce_a_unique_label() {
        let id = Uuid::new_v4();
        let (name, suffix) = rtsp_label_parts("   ", &id);
        assert!(name.is_empty());
        assert_eq!(
            vcam_label(&name, &suffix),
            format!("{LABEL_PREFIX} {suffix}")
        );
    }
}
