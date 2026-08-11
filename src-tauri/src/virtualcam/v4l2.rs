//! Backend Linux: v4l2loopback. Devices criados/removidos em runtime via
//! `v4l2loopback-ctl add/delete` (alocação dinâmica, `exclusive_caps=1` —
//! research.md R3); frames entregues por um subprocesso `ffmpeg -f v4l2`
//! por câmera (conversão RGBA→YUYV422 feita pelo próprio ffmpeg). Reaproveita
//! uma dependência já pinada do projeto em vez de bindings ioctl manuais não
//! verificáveis na plataforma onde este módulo foi escrito (Windows —
//! `#[cfg(target_os = "linux")]` deixa este arquivo fora do build ali).
//!
//! Formatos de `v4l2loopback-ctl` verificados contra o código-fonte real
//! (upstream v4l2loopback/v4l2loopback `utils/v4l2loopback-ctl.c`,
//! 2026-07-13): `add` sem device explícito cria um device livre e imprime
//! só `/dev/videoN\n`; `list` manda o cabeçalho só para stderr (stdout só
//! tem linhas de dado, formato `/dev/video%-3d\t/dev/video%-3d\t<name>`);
//! `--version` imprime `<progname> v<major>.<minor>.<bugfix>`.
//!
//! Fallback para `modprobe` quando `ctl_supports_dynamic_add` indica versão
//! < 0.13 (ou ctl ausente) ainda não é acionado por `create()` — T022 cobre
//! o caminho feliz; a decisão pura já está implementada e testada (T019).

use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, Command, Stdio};

use uuid::Uuid;

use crate::error::AppError;
use crate::model::{VirtualCamera, VirtualCameraState};
use crate::virtualcam::{standby_frame, VcamError, VirtualCameraBackend};

const CTL_BIN: &str = "v4l2loopback-ctl";
const FFMPEG_BIN: &str = "ffmpeg";

/// Um device listado por `v4l2loopback-ctl list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackDevice {
    pub output: String,
    pub capture: String,
    pub name: String,
}

/// Extrai o path do device criado da saída de `v4l2loopback-ctl add`
/// (só `/dev/videoN` no stdout).
pub fn parse_added_device(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    if line.starts_with("/dev/video") {
        Some(line.to_string())
    } else {
        None
    }
}

/// Monta os argumentos de `add`: sem device explícito (o kernel/ctl escolhe
/// um livre — R3 "alocação dinâmica") e com `exclusive_caps=1` (mandatório
/// para Chrome/WebRTC e OBS — R3).
pub fn build_add_args(label: &str) -> Vec<String> {
    vec![
        "add".to_string(),
        "-n".to_string(),
        label.to_string(),
        "-x".to_string(),
        "1".to_string(),
    ]
}

/// Parseia `v4l2loopback-ctl list` (stdout; o cabeçalho vai só para stderr
/// no binário real, então não há linha para pular aqui).
pub fn parse_list_output(stdout: &str) -> Vec<LoopbackDevice> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let output = fields.next()?.trim().to_string();
            let capture = fields.next()?.trim().to_string();
            let name = fields.next().unwrap_or("").trim().to_string();
            Some(LoopbackDevice {
                output,
                capture,
                name,
            })
        })
        .collect()
}

/// Escolhe um device loopback existente com o label do CamLink para reuso
/// (primeiro match exato). Sessões novas reaproveitam o MESMO `/dev/videoN`:
/// consumidores (OBS/Meet) mantêm a referência entre trocas de câmera e
/// restarts do app — criar um device por sessão fazia o Chrome acumular
/// duplicatas mortas ("câmera indisponível", exclusive_caps sem writer) e o
/// OBS congelar no último frame do device deletado.
pub fn find_reusable_device(devices: &[LoopbackDevice], label: &str) -> Option<String> {
    devices
        .iter()
        .find(|d| d.name == label)
        .map(|d| d.output.clone())
}

/// Parseia a versão de `v4l2loopback-ctl --version`/`-v`
/// (`<progname> v<major>.<minor>.<bugfix>`).
pub fn parse_ctl_version(stdout: &str) -> Option<(u32, u32, u32)> {
    // Primeiro token que fecha o formato `vX.Y.Z` completo — filtrar só por
    // prefixo "v" fazia o nome do próprio programa ("v4l2loopback-ctl") ser
    // tomado como token de versão, e o parse abortava ali.
    stdout
        .split_whitespace()
        .filter_map(|token| {
            let mut parts = token.strip_prefix('v')?.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let bugfix = parts.next()?.parse().ok()?;
            Some((major, minor, bugfix))
        })
        .next()
}

/// `v4l2loopback-ctl add` sem device explícito só é confiável (alocação
/// dinâmica de fato livre) a partir da 0.13 (research.md R3).
pub fn ctl_supports_dynamic_add(version: Option<(u32, u32, u32)>) -> bool {
    matches!(version, Some(v) if v >= (0, 13, 0))
}

/// Detecta o bloqueio clássico do Secure Boot ao carregar um módulo de
/// kernel não assinado (`modprobe: ... Key was rejected by service`).
pub fn detect_secure_boot_block(stderr: &str) -> Option<AppError> {
    if stderr
        .to_lowercase()
        .contains("key was rejected by service")
    {
        Some(
            AppError::new(
                "secure_boot_blocked",
                "O Secure Boot bloqueou o carregamento do módulo v4l2loopback",
            )
            .with_hint(
                "Assine o módulo (mokutil/DKMS) ou desative o Secure Boot no BIOS/UEFI \
                 — veja o guia de assinatura do módulo na documentação.",
            ),
        )
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// V4l2Backend
// ---------------------------------------------------------------------------

struct ManagedCamera {
    camera: VirtualCamera,
    ffmpeg: Child,
    resolution: (u32, u32),
}

impl Drop for ManagedCamera {
    fn drop(&mut self) {
        let _ = self.ffmpeg.kill();
        let _ = self.ffmpeg.wait();
    }
}

/// Backend real de câmera virtual via v4l2loopback: cria o device com
/// `v4l2loopback-ctl` e entrega frames por um subprocesso `ffmpeg -f v4l2`
/// dedicado por câmera.
pub struct V4l2Backend {
    cameras: HashMap<Uuid, ManagedCamera>,
}

impl Default for V4l2Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl V4l2Backend {
    pub fn new() -> Self {
        let mut backend = Self {
            cameras: HashMap::new(),
        };
        backend.cleanup_stale();
        backend
    }

    fn spawn_ffmpeg(
        device_path: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<Child, VcamError> {
        let (w, h) = resolution;
        Command::new(FFMPEG_BIN)
            .args([
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba",
                "-video_size",
                &format!("{w}x{h}"),
                "-framerate",
                &fps.to_string(),
                "-i",
                "pipe:0",
                "-pix_fmt",
                "yuyv422",
                "-f",
                "v4l2",
                device_path,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| VcamError::Backend(format!("falha ao iniciar ffmpeg: {e}")))
    }
}

impl V4l2Backend {
    fn list_devices() -> Vec<LoopbackDevice> {
        Command::new(CTL_BIN)
            .arg("list")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_list_output(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    }

    /// Remove duplicatas do `label` que não sejam `keep` (best-effort:
    /// `delete` falha com EBUSY se alguém ainda estiver usando o device —
    /// nesse caso ele fica pra trás e é limpo numa chamada futura, quando o
    /// consumidor tiver soltado).
    fn cleanup_duplicates(existing: &[LoopbackDevice], label: &str, keep: Option<&str>) {
        for extra in existing
            .iter()
            .filter(|d| d.name == label && Some(d.output.as_str()) != keep)
        {
            match Command::new(CTL_BIN)
                .args(["delete", &extra.output])
                .output()
            {
                Ok(o) if o.status.success() => {
                    tracing::info!(path = %extra.output, "device v4l2 duplicado removido");
                }
                _ => {
                    tracing::debug!(path = %extra.output, "duplicata em uso ou não removível — mantida")
                }
            }
        }
    }

    fn allocate_new_device(label: &str) -> Result<String, VcamError> {
        let output = Command::new(CTL_BIN)
            .args(build_add_args(label))
            .output()
            .map_err(|e| VcamError::Backend(format!("falha ao executar v4l2loopback-ctl: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if let Some(err) = detect_secure_boot_block(&stderr) {
                return Err(VcamError::Backend(err.msg));
            }
            return Err(VcamError::Backend(format!(
                "v4l2loopback-ctl add falhou: {stderr}"
            )));
        }
        parse_added_device(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| VcamError::Backend("saída inesperada de v4l2loopback-ctl add".into()))
    }

    fn finish_create(
        &mut self,
        device_path: String,
        label: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<VirtualCamera, VcamError> {
        let ffmpeg = Self::spawn_ffmpeg(&device_path, resolution, fps)?;
        let camera = VirtualCamera {
            id: Uuid::new_v4(),
            label: label.to_string(),
            backend_path: device_path,
            state: VirtualCameraState::Standby,
        };
        self.cameras.insert(
            camera.id,
            ManagedCamera {
                camera: camera.clone(),
                ffmpeg,
                resolution,
            },
        );
        tracing::info!(id = %camera.id, path = %camera.backend_path, "câmera virtual v4l2 criada");
        Ok(camera)
    }
}

impl VirtualCameraBackend for V4l2Backend {
    fn create(
        &mut self,
        label: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<VirtualCamera, VcamError> {
        // Reuso de device estável (ver `find_reusable_device`): duplicatas
        // do mesmo label (sessões antigas mortas sem cleanup) são removidas
        // best-effort — `delete` falha com EBUSY se alguém estiver usando,
        // o que já serve de guarda para uma segunda instância viva.
        let existing = Self::list_devices();
        let reusable = find_reusable_device(&existing, label);
        Self::cleanup_duplicates(&existing, label, reusable.as_deref());

        let device_path = if let Some(path) = reusable {
            tracing::info!(%path, "reusando device v4l2 existente");
            path
        } else {
            Self::allocate_new_device(label)?
        };

        self.finish_create(device_path, label, resolution, fps)
    }

    fn create_fresh(
        &mut self,
        label: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<VirtualCamera, VcamError> {
        // Sem lookup de reuso: o device antigo (se ainda existir) só é
        // removido best-effort — normalmente falha com EBUSY porque o
        // consumidor (Chrome/Meet) ainda está com ele aberto, e é isso
        // mesmo que queremos, já que reaproveitá-lo é o problema que este
        // método existe pra evitar.
        let existing = Self::list_devices();
        Self::cleanup_duplicates(&existing, label, None);
        let device_path = Self::allocate_new_device(label)?;
        tracing::info!(path = %device_path, "device v4l2 novo (resolução de saída mudou)");
        self.finish_create(device_path, label, resolution, fps)
    }

    fn feed_frame(&mut self, id: &Uuid, frame: &[u8]) -> Result<(), VcamError> {
        if frame.is_empty() {
            return Err(VcamError::InvalidFrame("frame vazio".into()));
        }
        let managed = self.cameras.get_mut(id).ok_or(VcamError::NotFound(*id))?;
        let expected = managed.resolution.0 as usize * managed.resolution.1 as usize * 4;
        if frame.len() != expected {
            return Err(VcamError::InvalidFrame(format!(
                "tamanho do frame ({}) não bate com a resolução declarada \
                 ({expected} bytes esperados, RGBA)",
                frame.len()
            )));
        }
        let stdin = managed
            .ffmpeg
            .stdin
            .as_mut()
            .ok_or_else(|| VcamError::Backend("stdin do ffmpeg indisponível".into()))?;
        stdin
            .write_all(frame)
            .map_err(|e| VcamError::Backend(format!("falha ao escrever frame: {e}")))?;
        managed.camera.state = VirtualCameraState::Live;
        Ok(())
    }

    fn set_standby(&mut self, id: &Uuid, message: &str) -> Result<(), VcamError> {
        let managed = self.cameras.get_mut(id).ok_or(VcamError::NotFound(*id))?;
        let frame = standby_frame(managed.resolution, message);
        let stdin = managed
            .ffmpeg
            .stdin
            .as_mut()
            .ok_or_else(|| VcamError::Backend("stdin do ffmpeg indisponível".into()))?;
        stdin
            .write_all(&frame)
            .map_err(|e| VcamError::Backend(format!("falha ao escrever frame de espera: {e}")))?;
        managed.camera.state = VirtualCameraState::Standby;
        Ok(())
    }

    fn destroy(&mut self, id: &Uuid) -> Result<(), VcamError> {
        let managed = self.cameras.remove(id).ok_or(VcamError::NotFound(*id))?;
        let device_path = managed.camera.backend_path.clone();
        drop(managed); // encerra o ffmpeg (Drop de ManagedCamera)

        // O device NÃO é deletado: ele é o alvo estável que consumidores
        // (OBS/Meet) mantêm aberto entre sessões e trocas de câmera — a
        // próxima sessão o reusa (`find_reusable_device`) e o writer novo
        // faz os frames voltarem no MESMO device. Equivale ao filtro
        // DirectShow do Windows, que permanece registrado entre sessões.
        tracing::info!(path = %device_path, "sessão encerrada; device v4l2 mantido para reuso");
        Ok(())
    }

    fn camera(&self, id: &Uuid) -> Option<&VirtualCamera> {
        self.cameras.get(id).map(|m| &m.camera)
    }

    fn cameras(&self) -> Vec<&VirtualCamera> {
        self.cameras.values().map(|m| &m.camera).collect()
    }

    fn cleanup_stale(&mut self) {
        let existing = Self::list_devices();
        let mut labels: Vec<&str> = existing.iter().map(|d| d.name.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        for label in labels {
            let keep = existing
                .iter()
                .find(|d| d.name == label)
                .map(|d| d.output.as_str());
            Self::cleanup_duplicates(&existing, label, keep);
        }
    }

    fn purge_all(&mut self) {
        let ids: Vec<Uuid> = self.cameras.keys().copied().collect();
        for id in ids {
            let Some(managed) = self.cameras.remove(&id) else {
                continue;
            };
            let device_path = managed.camera.backend_path.clone();
            drop(managed); // mata o ffmpeg (Drop de ManagedCamera)
            match Command::new(CTL_BIN)
                .args(["delete", &device_path])
                .output()
            {
                Ok(o) if o.status.success() => {
                    tracing::info!(path = %device_path, "device v4l2 removido ao fechar o app");
                }
                _ => {
                    tracing::debug!(path = %device_path, "falha ao remover device v4l2 ao fechar (talvez ainda em uso)")
                }
            }
        }
    }
}
