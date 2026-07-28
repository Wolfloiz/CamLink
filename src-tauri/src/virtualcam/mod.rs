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

use crate::model::{VirtualCamera, VirtualCameraState};

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
