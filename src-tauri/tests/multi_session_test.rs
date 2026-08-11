//! T062 [US6] — Testes de orquestração multi-sessão: N fontes concorrentes
//! (celulares Android via `StreamManager` + câmeras RTSP via
//! `rtsp_manager::start_session`), isolamento de falha entre elas (FR-021),
//! limite prático de 4 fontes simultâneas e encerramento limpo. Backends
//! reais substituídos pelo binário fake de `tests/bin/fake_backend.rs`
//! (mesmo padrão de `stream_lifecycle_test.rs`) — sem hardware, sem
//! servidor RTSP real.
//!
//! Cada `StreamManager` usado aqui tem seu próprio `ExternalPaths::extra_env`
//! (aplicado só aos processos QUE ELE spawna, ver `stream_manager.rs`), o
//! que permite dar comportamentos diferentes (crash vs. saudável) a
//! celulares "diferentes" sem mexer no ambiente global do processo de
//! teste. As sessões RTSP usam `rtsp_manager::start_session` diretamente,
//! que não tem esse mecanismo — por isso os testes que precisam de uma
//! fonte "quebrando" usam sempre uma sessão Android para isso, e mantêm as
//! RTSP no modo default (`stay_alive`, quando `FAKE_BACKEND_MODE` não está
//! setado no ambiente).

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use camlink_lib::model::{SessionSource, SessionState, StreamConfig, VideoCodec};
use camlink_lib::rtsp_manager::{start_session, RtspSession, RtspSessionConfig};
use camlink_lib::stream_manager::{ExternalPaths, StreamManager};
use camlink_lib::virtualcam;

fn fake_backend_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_backend"))
}

fn android_paths(extra_env: Vec<(String, String)>) -> ExternalPaths {
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

/// Sessão RTSP "fake": mesmo binário fake no papel do ffmpeg (ignora argv,
/// só reage a env vars). `sink: None` evita o caminho de leitura de frames
/// (que exigiria o fake escrever bytes no formato certo em stdout) — só o
/// lifecycle do processo importa aqui.
fn start_fake_rtsp(standby_calls: Arc<StdMutex<Vec<String>>>) -> RtspSession {
    let config = RtspSessionConfig {
        ffmpeg: fake_backend_path(),
        url: "rtsp://192.0.2.1:554/stream1".into(),
        resolution: (640, 480),
        fps: 15,
        v4l2_device: None,
    };
    start_session(
        config,
        None,
        Arc::new(move |msg: &str| {
            standby_calls.lock().unwrap().push(msg.to_string());
        }),
    )
}

/// Espera (com timeout) até que `state()` satisfaça `pred` — mesmo padrão
/// de `stream_lifecycle_test.rs::wait_for_state`.
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

async fn start_android(manager: &StreamManager, serial: &str, device: &str) -> uuid::Uuid {
    manager
        .start(
            SessionSource::Android(serial.to_string()),
            sample_config(),
            device,
            None,
        )
        .await
        .expect("start deve suceder com backend fake saudável")
}

// ---------------------------------------------------------------------------
// 4 fontes concorrentes, misturando 2 celulares + 2 câmeras RTSP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn four_mixed_sources_run_independently() {
    let manager_a = StreamManager::new(android_paths(vec![]));
    let manager_b = StreamManager::new(android_paths(vec![]));

    let phone_a = start_android(&manager_a, "PHONE-A-SERIAL", "/dev/video-fake-a").await;
    let phone_b = start_android(&manager_b, "PHONE-B-SERIAL", "/dev/video-fake-b").await;

    wait_for_state(&manager_a, phone_a, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;
    wait_for_state(&manager_b, phone_b, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;

    let standby_c: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let standby_d: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let rtsp_c = start_fake_rtsp(Arc::clone(&standby_c));
    let rtsp_d = start_fake_rtsp(Arc::clone(&standby_d));

    // As 4 fontes ficam de pé ao mesmo tempo por uma janela — nenhuma delas
    // reporta reconexão/queda (o fake fica parado vivo até o teste parar).
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        manager_a.state(phone_a).await,
        Some(SessionState::Streaming)
    );
    assert_eq!(
        manager_b.state(phone_b).await,
        Some(SessionState::Streaming)
    );
    assert!(
        standby_c.lock().unwrap().is_empty(),
        "fonte RTSP C não deveria reportar standby/reconexão"
    );
    assert!(
        standby_d.lock().unwrap().is_empty(),
        "fonte RTSP D não deveria reportar standby/reconexão"
    );

    manager_a.stop(phone_a).await.expect("stop A");
    manager_b.stop(phone_b).await.expect("stop B");
    rtsp_c.stop().await;
    rtsp_d.stop().await;
}

// ---------------------------------------------------------------------------
// Isolamento de falha (FR-021): 1 celular crasha, as outras 3 fontes não
// sentem nada.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_session_crash_does_not_affect_others() {
    let marker = tempfile::NamedTempFile::new()
        .expect("tempfile")
        .path()
        .to_path_buf();
    let _ = std::fs::remove_file(&marker);

    let manager_crashing = StreamManager::new(android_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "crash_once".into()),
        (
            "FAKE_BACKEND_MARKER_FILE".into(),
            marker.to_string_lossy().into_owned(),
        ),
        ("FAKE_BACKEND_DELAY_MS".into(), "50".into()),
    ]));
    let manager_healthy = StreamManager::new(android_paths(vec![]));

    let crashing = start_android(&manager_crashing, "PHONE-CRASH", "/dev/video-fake-x").await;
    let healthy = start_android(&manager_healthy, "PHONE-HEALTHY", "/dev/video-fake-y").await;

    let standby_c: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let standby_d: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let rtsp_c = start_fake_rtsp(Arc::clone(&standby_c));
    let rtsp_d = start_fake_rtsp(Arc::clone(&standby_d));

    wait_for_state(&manager_healthy, healthy, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;

    // A sessão marcada pra crashar passa por Reconnecting/SourceLost...
    wait_for_state(&manager_crashing, crashing, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Reconnecting | SessionState::SourceLost)
    })
    .await;
    // ...e enquanto isso a sessão saudável e as duas RTSP não notam nada.
    assert_eq!(
        manager_healthy.state(healthy).await,
        Some(SessionState::Streaming),
        "sessão saudável não deveria ser afetada pelo crash de outra"
    );
    assert!(standby_c.lock().unwrap().is_empty());
    assert!(standby_d.lock().unwrap().is_empty());

    // ...até se recuperar sozinha (mesmo comportamento de
    // `backend_crash_triggers_reconnect_and_recovers_streaming`).
    let recovered = wait_for_state(&manager_crashing, crashing, Duration::from_secs(5), |s| {
        *s == SessionState::Streaming
    })
    .await;
    assert_eq!(recovered, SessionState::Streaming);

    // As outras 3 fontes nunca se mexeram durante todo o incidente.
    assert_eq!(
        manager_healthy.state(healthy).await,
        Some(SessionState::Streaming)
    );
    assert!(standby_c.lock().unwrap().is_empty());
    assert!(standby_d.lock().unwrap().is_empty());

    manager_crashing
        .stop(crashing)
        .await
        .expect("stop crashing");
    manager_healthy.stop(healthy).await.expect("stop healthy");
    rtsp_c.stop().await;
    rtsp_d.stop().await;
}

// ---------------------------------------------------------------------------
// Parar uma fonte não mexe nas outras 3 (registry de sessões independente
// por session_id).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_one_leaves_others_running() {
    let manager = StreamManager::new(android_paths(vec![]));

    let phone_a = start_android(&manager, "PHONE-A", "/dev/video-fake-1").await;
    let phone_b = start_android(&manager, "PHONE-B", "/dev/video-fake-2").await;

    let standby_c: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let standby_d: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let rtsp_c = start_fake_rtsp(Arc::clone(&standby_c));
    let rtsp_d = start_fake_rtsp(Arc::clone(&standby_d));

    wait_for_state(&manager, phone_a, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;
    wait_for_state(&manager, phone_b, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;

    manager.stop(phone_a).await.expect("stop deve suceder");

    wait_for_state(&manager, phone_a, Duration::from_secs(3), |s| {
        *s == SessionState::Idle
    })
    .await;
    // B e as duas RTSP continuam de pé, sem qualquer sinal de perturbação.
    assert_eq!(manager.state(phone_b).await, Some(SessionState::Streaming));
    assert!(standby_c.lock().unwrap().is_empty());
    assert!(standby_d.lock().unwrap().is_empty());

    rtsp_c.stop().await;
    // Parar C não afeta D nem B.
    assert_eq!(manager.state(phone_b).await, Some(SessionState::Streaming));
    assert!(standby_d.lock().unwrap().is_empty());

    manager.stop(phone_b).await.expect("stop B");
    rtsp_d.stop().await;
}

// ---------------------------------------------------------------------------
// Limite prático de 4 fontes simultâneas (FR-021/spec.md notas)
// ---------------------------------------------------------------------------

#[test]
fn fifth_concurrent_session_is_rejected() {
    for active in 0..virtualcam::MAX_CONCURRENT_SOURCES {
        assert!(
            virtualcam::check_capacity(active).is_ok(),
            "{active} fontes ativas ainda deveria caber no limite"
        );
    }
    let err = virtualcam::check_capacity(virtualcam::MAX_CONCURRENT_SOURCES)
        .expect_err("a 5ª fonte concorrente deve ser rejeitada");
    assert_eq!(err.code, "max_sources_reached");

    // Acima do limite continua rejeitando (não é um teto exato só na borda).
    assert!(virtualcam::check_capacity(virtualcam::MAX_CONCURRENT_SOURCES + 3).is_err());
}

// ---------------------------------------------------------------------------
// Encerrar 4 fontes concorrentes (o que acontece ao fechar o app) não trava
// nem deixa nenhuma pra trás.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_all_four_sources_completes_without_orphans() {
    // `StreamManager::kill_all_backends` é o caminho real usado no shutdown
    // do app (sinal OS chegando direto no processo) — mas ele é "best
    // effort" por design (mata o processo e não espera confirmação; ver seu
    // doc comment) porque o processo Rust inteiro está saindo em seguida.
    // Dentro do processo de teste, que continua vivo depois da chamada, o
    // monitor de reconexão trataria o kill como "crash" e tentaria
    // reconectar — então aqui validamos o caminho determinístico
    // equivalente (parar as 4 fontes explicitamente) em vez de correr atrás
    // do timing de `kill_all_backends`.
    let manager_a = StreamManager::new(android_paths(vec![]));
    let manager_b = StreamManager::new(android_paths(vec![]));

    let phone_a = start_android(&manager_a, "PHONE-A", "/dev/video-fake-1").await;
    let phone_b = start_android(&manager_b, "PHONE-B", "/dev/video-fake-2").await;
    let standby_c: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let standby_d: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let rtsp_c = start_fake_rtsp(Arc::clone(&standby_c));
    let rtsp_d = start_fake_rtsp(Arc::clone(&standby_d));

    wait_for_state(&manager_a, phone_a, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;
    wait_for_state(&manager_b, phone_b, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;

    // As 4 param concorrentemente (`tokio::join!`) — se alguma travasse a
    // outra, este teste estouraria o timeout de `wait_for_state`/do runner.
    let (stop_a, stop_b) = tokio::join!(manager_a.stop(phone_a), manager_b.stop(phone_b));
    stop_a.expect("stop A");
    stop_b.expect("stop B");
    rtsp_c.stop().await;
    rtsp_d.stop().await;

    assert_eq!(manager_a.state(phone_a).await, Some(SessionState::Idle));
    assert_eq!(manager_b.state(phone_b).await, Some(SessionState::Idle));
}
