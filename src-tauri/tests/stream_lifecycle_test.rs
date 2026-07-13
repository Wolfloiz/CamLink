//! T018 — Testes de lifecycle de `StreamManager` com um binário fake no
//! lugar do backend real (cliente scrcpy no Linux; bootstrap adb no Windows
//! — research.md R12). Cobre start/stop, crash→Reconnecting→retomada
//! (FR-005/006) e stderr → erro acionável (FR-010). Escritos ANTES da
//! implementação (Princípio III); falham até T024 implementar
//! `src-tauri/src/stream_manager.rs`.

use std::path::PathBuf;
use std::time::Duration;

use camlink_lib::model::{SessionSource, SessionState, StreamConfig, VideoCodec};
use camlink_lib::stream_manager::{classify_stderr, ExternalPaths, StreamManager};

fn fake_backend_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_backend"))
}

fn base_paths(extra_env: Vec<(String, String)>) -> ExternalPaths {
    let fake = fake_backend_path();
    ExternalPaths {
        adb: fake.clone(),
        scrcpy: fake,
        ffmpeg: PathBuf::from("ffmpeg"),
        server_jar: PathBuf::from("scrcpy-server.jar"),
        extra_env,
    }
}

fn sample_config() -> StreamConfig {
    StreamConfig {
        resolution: (1920, 1080),
        fps: 30,
        bitrate: 8_000_000,
        codec: VideoCodec::H264,
        camera_id: "0".into(),
    }
}

/// Espera (com timeout) até que `state()` satisfaça `pred`, para não
/// depender de sleeps fixos numa máquina de estados dirigida por subprocesso
/// real.
async fn wait_for_state<F>(
    manager: &StreamManager,
    session_id: uuid::Uuid,
    timeout: Duration,
    pred: F,
) -> SessionState
where
    F: Fn(&SessionState) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(state) = manager.state(session_id).await {
            if pred(&state) {
                return state;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timeout esperando estado esperado; último estado: {:?}",
                manager.state(session_id).await
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn start_reaches_streaming_then_stop_reaches_idle() {
    let manager = StreamManager::new(base_paths(vec![(
        "FAKE_BACKEND_MODE".into(),
        "stay_alive".into(),
    )]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder com backend fake saudável");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;

    manager.stop(session_id).await.expect("stop deve suceder");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        *s == SessionState::Idle
    })
    .await;
}

#[tokio::test]
async fn backend_crash_triggers_reconnect_and_recovers_streaming() {
    let marker = tempfile::NamedTempFile::new()
        .expect("tempfile")
        .path()
        .to_path_buf();
    // O arquivo não deve existir ainda: a primeira execução do fake cria o
    // marcador e crasha; a segunda (retomada) o encontra e fica de pé.
    let _ = std::fs::remove_file(&marker);

    let manager = StreamManager::new(base_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "crash_once".into()),
        (
            "FAKE_BACKEND_MARKER_FILE".into(),
            marker.to_string_lossy().into_owned(),
        ),
        ("FAKE_BACKEND_DELAY_MS".into(), "50".into()),
    ]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder mesmo que o backend vá crashar em seguida");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Reconnecting | SessionState::SourceLost)
    })
    .await;

    let final_state = wait_for_state(&manager, session_id, Duration::from_secs(5), |s| {
        *s == SessionState::Streaming
    })
    .await;
    assert_eq!(final_state, SessionState::Streaming);
}

#[tokio::test]
async fn stop_while_reconnecting_reaches_idle() {
    // Antes da correção de SessionControl, `stop()` só sabia mandar o
    // evento `Stop` (válido apenas a partir de `Streaming` na máquina de
    // estados) e só olhava `RunningSession.child` diretamente — que fica
    // `None` a maior parte do tempo (o processo real fica "emprestado"
    // dentro da task de monitor). Parar durante `Reconnecting` batia nos
    // dois problemas ao mesmo tempo: falhava a transição E não achava
    // processo nenhum para matar.
    let marker = tempfile::NamedTempFile::new()
        .expect("tempfile")
        .path()
        .to_path_buf();
    let _ = std::fs::remove_file(&marker);

    let manager = StreamManager::new(base_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "crash_once".into()),
        (
            "FAKE_BACKEND_MARKER_FILE".into(),
            marker.to_string_lossy().into_owned(),
        ),
        ("FAKE_BACKEND_DELAY_MS".into(), "50".into()),
    ]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder mesmo que o backend vá crashar em seguida");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Reconnecting | SessionState::SourceLost)
    })
    .await;

    manager
        .stop(session_id)
        .await
        .expect("stop deve suceder mesmo durante reconexão");

    assert_eq!(manager.state(session_id).await, Some(SessionState::Idle));
}

#[tokio::test]
async fn known_fatal_stderr_maps_to_actionable_error_without_retry() {
    let manager = StreamManager::new(base_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "stderr_error".into()),
        (
            "FAKE_BACKEND_STDERR_LINE".into(),
            "error: device unauthorized".into(),
        ),
    ]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder; o erro chega via stderr assíncrono");

    let state = wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Error(_))
    })
    .await;
    match state {
        SessionState::Error(msg) => assert!(
            !msg.is_empty(),
            "estado Error deve carregar mensagem acionável"
        ),
        other => panic!("esperava Error, obteve {other:?}"),
    }

    // Erro fatal não deve entrar em retry automático: o estado permanece
    // Error mesmo após esperar além do tempo de uma reconexão normal.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(matches!(
        manager.state(session_id).await,
        Some(SessionState::Error(_))
    ));
}

#[tokio::test]
async fn stop_on_unknown_session_is_actionable_error() {
    let manager = StreamManager::new(base_paths(vec![]));
    let err = manager
        .stop(uuid::Uuid::new_v4())
        .await
        .expect_err("stop de sessão inexistente deve falhar");
    assert!(!err.msg.is_empty());
}

// ---------------------------------------------------------------------------
// classify_stderr — mapeamento de linhas conhecidas do adb/scrcpy para
// AppError acionável (FR-010); erros desconhecidos ficam None (tratados como
// perda transitória de sinal pelo monitor de lifecycle).
// ---------------------------------------------------------------------------

#[test]
fn classify_stderr_recognizes_unauthorized() {
    let err = classify_stderr("error: device unauthorized").expect("deve classificar");
    assert_eq!(err.code, "device_unauthorized");
    assert!(err.action_hint.is_some());
}

#[test]
fn classify_stderr_recognizes_device_not_found() {
    let err = classify_stderr("adb: no devices/emulators found").expect("deve classificar");
    assert_eq!(err.code, "device_not_found");
}

#[test]
fn classify_stderr_recognizes_version_mismatch() {
    let err =
        classify_stderr("adb server version doesn't match this client").expect("deve classificar");
    assert_eq!(err.code, "scrcpy_version_mismatch");
}

#[test]
fn classify_stderr_ignores_unknown_lines() {
    assert!(classify_stderr("INFO: Texture: 1920x1080").is_none());
    assert!(classify_stderr("").is_none());
}
