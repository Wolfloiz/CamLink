//! Detecção de dispositivos Android: `adb devices -l` com polling e gate de
//! compatibilidade Android 12+ (FR-001/FR-002/FR-002a, research.md R9).
//! v1: polling universal de 500 ms (Linux e Windows); rescan acelerado por
//! udev no Linux fica para uma iteração futura (R9 já prevê o fallback
//! como suficiente para o SC-001 de ≤ 3 s).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

use crate::error::AppError;
use crate::model::{AndroidDevice, AuthState};

/// SDK mínimo suportado: Android 12 (API 31) — FR-002a.
const MIN_SDK: u32 = 31;

/// Eventos de hotplug emitidos ao comparar duas leituras de `adb devices -l`
/// (contracts/tauri-commands.md: `device_connected`/`device_disconnected`/
/// `device_unauthorized`).
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceEvent {
    Connected(AndroidDevice),
    Disconnected(String),
    Unauthorized(String),
}

/// Parseia a saída de `adb devices -l`. Não preenche o gate de
/// compatibilidade (`compatible`/`incompat_reason`) — depende de uma
/// chamada adicional (`getprop ro.build.version.sdk`), feita por
/// [`poll_devices`], indisponível nesta única linha de texto.
pub fn parse_devices_l(output: &str) -> Vec<AndroidDevice> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "List of devices attached")
        .filter_map(parse_device_line)
        .collect()
}

fn parse_device_line(line: &str) -> Option<AndroidDevice> {
    let mut parts = line.split_whitespace();
    let serial = parts.next()?.to_string();
    let state = parts.next()?;
    let auth_state = match state {
        "device" => AuthState::Authorized,
        "unauthorized" => AuthState::Unauthorized,
        // "offline" e outros estados transitórios do adb (ex.: "sideload",
        // "recovery") viram offline: não utilizáveis, sem exigir
        // autorização do usuário.
        _ => AuthState::Offline,
    };

    let model = parts
        .filter_map(|field| field.strip_prefix("model:"))
        .next()
        .map(str::to_string)
        .unwrap_or_else(|| serial.clone());

    Some(AndroidDevice {
        serial,
        model,
        auth_state,
        compatible: true,
        incompat_reason: None,
        capabilities: None,
    })
}

/// Classifica a SDK do Android contra o mínimo suportado (FR-002a):
/// `(compatible, motivo_se_incompatível)`.
pub fn classify_sdk(sdk: u32) -> (bool, Option<String>) {
    if sdk >= MIN_SDK {
        (true, None)
    } else {
        (
            false,
            Some(format!(
                "Android muito antigo (API {sdk}); o CamLink requer Android 12 \
                 (API {MIN_SDK}) ou superior"
            )),
        )
    }
}

/// Compara duas leituras de `adb devices -l` e produz os eventos de
/// hotplug correspondentes (dispositivos novos, removidos ou com mudança de
/// estado de autorização).
pub fn diff_devices(previous: &[AndroidDevice], current: &[AndroidDevice]) -> Vec<DeviceEvent> {
    use std::collections::HashMap;

    let prev_by_serial: HashMap<&str, &AndroidDevice> =
        previous.iter().map(|d| (d.serial.as_str(), d)).collect();
    let curr_by_serial: HashMap<&str, &AndroidDevice> =
        current.iter().map(|d| (d.serial.as_str(), d)).collect();

    let mut events = Vec::new();

    for device in current {
        let changed = match prev_by_serial.get(device.serial.as_str()) {
            None => true,
            Some(prev) => prev.auth_state != device.auth_state,
        };
        if !changed {
            continue;
        }
        match device.auth_state {
            AuthState::Authorized => events.push(DeviceEvent::Connected(device.clone())),
            AuthState::Unauthorized => {
                events.push(DeviceEvent::Unauthorized(device.serial.clone()))
            }
            AuthState::Offline => {
                // Só é desconexão de fato se já conhecíamos o dispositivo;
                // primeira aparição offline é transitória (ex.:
                // reconectando) e não gera evento.
                if prev_by_serial.contains_key(device.serial.as_str()) {
                    events.push(DeviceEvent::Disconnected(device.serial.clone()));
                }
            }
        }
    }

    for device in previous {
        if !curr_by_serial.contains_key(device.serial.as_str()) {
            events.push(DeviceEvent::Disconnected(device.serial.clone()));
        }
    }

    events
}

async fn query_sdk_version(adb_path: &Path, serial: &str) -> Result<u32, AppError> {
    let output = crate::procutil::hide_console(Command::new(adb_path))
        .args(["-s", serial, "shell", "getprop", "ro.build.version.sdk"])
        .output()
        .await
        .map_err(|e| AppError::new("adb_exec_failed", format!("Falha ao executar adb: {e}")))?;

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|_| {
            AppError::new(
                "sdk_parse_failed",
                "Não foi possível ler a versão do Android do dispositivo",
            )
        })
}

/// Executa `adb devices -l`, aplica o gate Android 12+ (FR-002a) nos
/// dispositivos autorizados e retorna os eventos de hotplug em relação a
/// `previous` (que é atualizado in-place para a próxima chamada).
pub async fn poll_devices(
    adb_path: &Path,
    previous: &mut Vec<AndroidDevice>,
) -> Result<Vec<DeviceEvent>, AppError> {
    let output = crate::procutil::hide_console(Command::new(adb_path))
        .args(["devices", "-l"])
        .output()
        .await
        .map_err(|e| AppError::new("adb_exec_failed", format!("Falha ao executar adb: {e}")))?;

    if !output.status.success() {
        return Err(AppError::new(
            "adb_devices_failed",
            format!(
                "adb devices -l retornou erro: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        )
        .with_hint("Verifique se o adb está instalado e acessível no PATH."));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current = parse_devices_l(&stdout);

    for device in current.iter_mut() {
        if device.auth_state != AuthState::Authorized {
            continue;
        }
        match query_sdk_version(adb_path, &device.serial).await {
            Ok(sdk) => {
                let (compatible, reason) = classify_sdk(sdk);
                device.compatible = compatible;
                device.incompat_reason = reason;
            }
            Err(err) => {
                // Sem SDK conhecida, não travamos a listagem: o
                // dispositivo aparece, mas sem garantia de compatibilidade.
                tracing::debug!(serial = %device.serial, error = %err, "getprop sdk falhou");
            }
        }
    }

    let events = diff_devices(previous, &current);
    *previous = current;
    Ok(events)
}

/// Loop de polling contínuo (500 ms — FR-001: detecção em ≤ 3 s) que invoca
/// `on_event` para cada mudança de hotplug. Roda indefinidamente; deve ser
/// disparado numa task própria pelo chamador (T025).
pub async fn run_polling_loop(adb_path: PathBuf, on_event: impl Fn(DeviceEvent) + Send + 'static) {
    let mut previous: Vec<AndroidDevice> = Vec::new();
    loop {
        match poll_devices(&adb_path, &mut previous).await {
            Ok(events) => {
                for event in events {
                    on_event(event);
                }
            }
            Err(err) => {
                tracing::error!(code = %err.code, msg = %err.msg, "poll_devices falhou");
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
