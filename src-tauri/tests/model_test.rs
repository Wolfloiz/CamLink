//! T008 — Testes dos tipos de domínio (data-model.md): serialização/roundtrip
//! e transições de estado de `StreamSession`.
//!
//! Escritos ANTES da implementação (Constituição, Princípio III). Devem falhar
//! até T011 implementar `src-tauri/src/model.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use camlink_lib::model::{
    AndroidDevice, AppConfig, AuthState, CameraFacing, CameraInfo, ControlState,
    DeviceCapabilities, FocusMode, ManualExposure, RawCaptureJob, RawInfo, RawJobKind, RawJobState,
    RtspSource, RtspState, SessionEvent, SessionSource, SessionState, SessionStats, SmartMode,
    StreamConfig, StreamSession, VideoCodec, VirtualCamera, VirtualCameraState, WbMode,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn sample_capabilities() -> DeviceCapabilities {
    DeviceCapabilities {
        cameras: vec![
            CameraInfo {
                id: "0".into(),
                facing: CameraFacing::Back,
                max_resolution: (4032, 3024),
                fps_ranges: vec![(15, 30), (30, 30), (60, 60)],
            },
            CameraInfo {
                id: "1".into(),
                facing: CameraFacing::Front,
                max_resolution: (1920, 1080),
                fps_ranges: vec![(30, 30)],
            },
        ],
        zoom_range: (1.0, 8.0),
        iso_range: Some((50, 3200)),
        exposure_comp_range: (-4, 4),
        wb_modes: vec![WbMode::Auto, WbMode::Daylight, WbMode::Cloudy],
        supports_eis: true,
        supports_torch: true,
        raw: Some(RawInfo {
            sensor_size: (4032, 3024),
            frame_bytes: 24_385_536,
        }),
    }
}

fn sample_device() -> AndroidDevice {
    AndroidDevice {
        serial: "R58M12ABCDE".into(),
        model: "Pixel 7".into(),
        auth_state: AuthState::Authorized,
        compatible: true,
        incompat_reason: None,
        capabilities: Some(sample_capabilities()),
    }
}

fn sample_stream_config() -> StreamConfig {
    StreamConfig {
        resolution: (1920, 1080),
        fps: 30,
        bitrate: 8_000_000,
        codec: VideoCodec::H264,
        camera_id: "0".into(),
    }
}

fn sample_session() -> StreamSession {
    StreamSession {
        source: SessionSource::Android("R58M12ABCDE".into()),
        virtual_camera: Uuid::new_v4(),
        config: sample_stream_config(),
        state: SessionState::Idle,
        stats: SessionStats::default(),
    }
}

fn roundtrip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let json = serde_json::to_string(value).expect("serialização falhou");
    serde_json::from_str(&json).expect("desserialização falhou")
}

// ---------------------------------------------------------------------------
// Serialização / roundtrip
// ---------------------------------------------------------------------------

#[test]
fn android_device_roundtrip() {
    let device = sample_device();
    let back = roundtrip(&device);
    assert_eq!(back, device);
}

#[test]
fn android_device_incompatible_carries_reason() {
    let device = AndroidDevice {
        serial: "OLD123".into(),
        model: "Galaxy S8".into(),
        auth_state: AuthState::Offline,
        compatible: false,
        incompat_reason: Some("Android 9 detectado; requer Android 12+".into()),
        capabilities: None,
    };
    let back = roundtrip(&device);
    assert_eq!(back, device);
    assert!(!back.compatible);
    assert!(back.incompat_reason.is_some());
}

#[test]
fn auth_state_all_variants_roundtrip() {
    for state in [
        AuthState::Authorized,
        AuthState::Unauthorized,
        AuthState::Offline,
    ] {
        assert_eq!(roundtrip(&state), state);
    }
}

#[test]
fn device_capabilities_roundtrip_preserves_optional_fields() {
    let caps = sample_capabilities();
    let back = roundtrip(&caps);
    assert_eq!(back, caps);

    // Sem ISO manual e sem RAW: Options ausentes sobrevivem ao roundtrip.
    let minimal = DeviceCapabilities {
        iso_range: None,
        raw: None,
        ..sample_capabilities()
    };
    let back = roundtrip(&minimal);
    assert!(back.iso_range.is_none());
    assert!(back.raw.is_none());
}

#[test]
fn rtsp_source_roundtrip_never_holds_plaintext_password() {
    let source = RtspSource {
        id: Uuid::new_v4(),
        name: "Câmera do quintal".into(),
        url: "rtsp://192.168.1.50:554/stream1".into(),
        secret_ref: Some("camlink/rtsp/cam-quintal".into()),
        state: RtspState::Idle,
    };
    let json = serde_json::to_string(&source).expect("serialização falhou");
    let back: RtspSource = serde_json::from_str(&json).expect("desserialização falhou");
    assert_eq!(back, source);

    // FR-018a: a URL persistida não contém credenciais e não existe campo de
    // senha — apenas a referência ao cofre do SO.
    assert!(!json.to_lowercase().contains("password"));
    assert!(!back.url.contains('@'));
}

#[test]
fn rtsp_state_error_variant_roundtrip() {
    let state = RtspState::Error("autenticação recusada (401)".into());
    assert_eq!(roundtrip(&state), state);
}

#[test]
fn virtual_camera_roundtrip() {
    let cam = VirtualCamera {
        id: Uuid::new_v4(),
        label: "CamLink Android".into(),
        backend_path: "/dev/video10".into(),
        state: VirtualCameraState::Standby,
    };
    assert_eq!(roundtrip(&cam), cam);
}

#[test]
fn stream_session_roundtrip_android_and_rtsp_sources() {
    let android = sample_session();
    assert_eq!(roundtrip(&android), android);

    let rtsp = StreamSession {
        source: SessionSource::Rtsp(Uuid::new_v4()),
        ..sample_session()
    };
    assert_eq!(roundtrip(&rtsp), rtsp);
}

#[test]
fn stream_config_codec_variants_roundtrip() {
    for codec in [VideoCodec::H264, VideoCodec::H265] {
        let config = StreamConfig {
            codec,
            ..sample_stream_config()
        };
        assert_eq!(roundtrip(&config), config);
    }
}

#[test]
fn session_state_error_roundtrip_keeps_actionable_message() {
    let state = SessionState::Error(
        "scrcpy encerrou inesperadamente; reconecte o cabo USB e tente novamente".into(),
    );
    let back = roundtrip(&state);
    match &back {
        SessionState::Error(msg) => assert!(msg.contains("reconecte o cabo")),
        other => panic!("esperava Error, obtive {other:?}"),
    }
    assert_eq!(back, state);
}

#[test]
fn control_state_roundtrip_all_focus_modes() {
    for focus in [
        FocusMode::ContinuousAuto,
        FocusMode::Tap { x: 0.25, y: 0.75 },
        FocusMode::Manual { distance: 1.5 },
    ] {
        let control = ControlState {
            focus,
            ..ControlState::default()
        };
        assert_eq!(roundtrip(&control), control);
    }
}

#[test]
fn control_state_manual_exposure_roundtrip() {
    let control = ControlState {
        mode: SmartMode::Pro,
        manual_exposure: Some(ManualExposure {
            iso: 800,
            exposure_time_ns: 16_666_667,
        }),
        ..ControlState::default()
    };
    assert_eq!(roundtrip(&control), control);
}

#[test]
fn raw_capture_job_roundtrip_all_states() {
    let jobs = [
        RawCaptureJob {
            kind: RawJobKind::Snapshot,
            output_dir: PathBuf::from("/home/user/Imagens/CamLink"),
            state: RawJobState::Done,
        },
        RawCaptureJob {
            kind: RawJobKind::Sequence { target_fps: 3.0 },
            output_dir: PathBuf::from("/home/user/Imagens/CamLink"),
            state: RawJobState::Running {
                frames: 12,
                bytes: 292_626_432,
                effective_fps: 1.8,
            },
        },
        RawCaptureJob {
            kind: RawJobKind::Snapshot,
            output_dir: PathBuf::from("/tmp/raw"),
            state: RawJobState::Failed("banda ADB insuficiente".into()),
        },
    ];
    for job in jobs {
        assert_eq!(roundtrip(&job), job);
    }
}

#[test]
fn app_config_roundtrip() {
    let mut last_stream_config = HashMap::new();
    last_stream_config.insert("R58M12ABCDE".to_string(), sample_stream_config());
    let config = AppConfig {
        rtsp_sources: vec![RtspSource {
            id: Uuid::new_v4(),
            name: "Portaria".into(),
            url: "rtsp://10.0.0.9/live".into(),
            secret_ref: None,
            state: RtspState::Idle,
        }],
        last_stream_config,
        raw_output_dir: Some(PathBuf::from("/home/user/Imagens/CamLink")),
        auto_connect: vec!["R58M12ABCDE".into()],
        device_nicknames: HashMap::from([(
            "R58M12ABCDE".to_string(),
            "Câmera lateral".to_string(),
        )]),
    };
    assert_eq!(roundtrip(&config), config);
}

// ---------------------------------------------------------------------------
// Defaults (data-model.md — ControlState)
// ---------------------------------------------------------------------------

#[test]
fn control_state_defaults_match_data_model() {
    let control = ControlState::default();
    assert_eq!(control.mode, SmartMode::Auto);
    assert_eq!(control.zoom_ratio, 1.0);
    assert_eq!(control.focus, FocusMode::ContinuousAuto);
    assert_eq!(control.exposure_comp, 0);
    assert!(control.manual_exposure.is_none());
    assert_eq!(control.wb_mode, WbMode::Auto);
    assert!(!control.torch);
}

#[test]
fn session_stats_default_is_zeroed() {
    let stats = SessionStats::default();
    assert_eq!(stats.fps, 0.0);
    assert_eq!(stats.uptime_secs, 0);
    assert_eq!(stats.reconnects, 0);
}

// ---------------------------------------------------------------------------
// Máquina de estados de StreamSession (data-model.md — Transições)
//
//   Idle → Starting → Streaming → Stopping → Idle
//                   Streaming → SourceLost → Reconnecting → Streaming
//                   Reconnecting → (timeout/cancel) → Idle
//   qualquer → Error(msg) → Idle
// ---------------------------------------------------------------------------

#[test]
fn happy_path_start_stream_stop() {
    let state = SessionState::Idle;
    let state = state.transition(SessionEvent::Start).expect("Idle+Start");
    assert_eq!(state, SessionState::Starting);
    let state = state
        .transition(SessionEvent::Started)
        .expect("Starting+Started");
    assert_eq!(state, SessionState::Streaming);
    let state = state
        .transition(SessionEvent::Stop)
        .expect("Streaming+Stop");
    assert_eq!(state, SessionState::Stopping);
    let state = state
        .transition(SessionEvent::Stopped)
        .expect("Stopping+Stopped");
    assert_eq!(state, SessionState::Idle);
}

#[test]
fn source_lost_then_reconnect_resumes_streaming() {
    let state = SessionState::Streaming
        .transition(SessionEvent::SourceLost)
        .expect("Streaming+SourceLost");
    assert_eq!(state, SessionState::SourceLost);
    let state = state
        .transition(SessionEvent::ReconnectStarted)
        .expect("SourceLost+ReconnectStarted");
    assert_eq!(state, SessionState::Reconnecting);
    let state = state
        .transition(SessionEvent::Recovered)
        .expect("Reconnecting+Recovered");
    assert_eq!(state, SessionState::Streaming);
}

#[test]
fn reconnecting_timeout_and_cancel_return_to_idle() {
    let reconnecting = SessionState::Reconnecting;
    assert_eq!(
        reconnecting
            .clone()
            .transition(SessionEvent::ReconnectTimeout)
            .expect("Reconnecting+Timeout"),
        SessionState::Idle
    );
    assert_eq!(
        reconnecting
            .transition(SessionEvent::Cancel)
            .expect("Reconnecting+Cancel"),
        SessionState::Idle
    );
}

#[test]
fn any_state_can_fail_with_actionable_message() {
    let msg = "adb não encontrado; instale android-tools".to_string();
    for state in [
        SessionState::Idle,
        SessionState::Starting,
        SessionState::Streaming,
        SessionState::Stopping,
        SessionState::SourceLost,
        SessionState::Reconnecting,
    ] {
        let failed = state
            .transition(SessionEvent::Fail(msg.clone()))
            .expect("qualquer estado + Fail");
        assert_eq!(failed, SessionState::Error(msg.clone()));
    }
}

#[test]
fn error_state_resets_to_idle() {
    let state = SessionState::Error("qualquer erro".into())
        .transition(SessionEvent::Reset)
        .expect("Error+Reset");
    assert_eq!(state, SessionState::Idle);
}

#[test]
fn invalid_transitions_are_rejected() {
    // Pares (estado, evento) que NÃO constam do diagrama de transições.
    let invalid: Vec<(SessionState, SessionEvent)> = vec![
        (SessionState::Idle, SessionEvent::Stop),
        (SessionState::Idle, SessionEvent::Started),
        (SessionState::Idle, SessionEvent::SourceLost),
        (SessionState::Starting, SessionEvent::Start),
        (SessionState::Streaming, SessionEvent::Start),
        (SessionState::Streaming, SessionEvent::Recovered),
        (SessionState::Stopping, SessionEvent::Start),
        (SessionState::Reconnecting, SessionEvent::Start),
        (SessionState::Error("x".into()), SessionEvent::Start),
        (SessionState::Error("x".into()), SessionEvent::Stop),
    ];
    for (state, event) in invalid {
        let result = state.clone().transition(event.clone());
        assert!(
            result.is_err(),
            "transição inválida aceita: {state:?} + {event:?}"
        );
    }
}

#[test]
fn invalid_transition_error_is_descriptive() {
    let err = SessionState::Idle
        .transition(SessionEvent::Stop)
        .expect_err("Idle+Stop deve falhar");
    let msg = err.to_string();
    assert!(
        msg.contains("Idle"),
        "erro deve citar o estado de origem: {msg}"
    );
    assert!(msg.contains("Stop"), "erro deve citar o evento: {msg}");
}

#[test]
fn stream_session_apply_event_updates_state_in_place() {
    let mut session = sample_session();
    session
        .apply(SessionEvent::Start)
        .expect("Idle+Start na sessão");
    assert_eq!(session.state, SessionState::Starting);
    session
        .apply(SessionEvent::Started)
        .expect("Starting+Started na sessão");
    assert_eq!(session.state, SessionState::Streaming);

    // Evento inválido não corrompe o estado atual.
    let before = session.state.clone();
    let result = session.apply(SessionEvent::Started);
    assert!(result.is_err());
    assert_eq!(session.state, before);
}
