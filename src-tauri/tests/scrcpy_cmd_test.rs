//! T020 — Testes de montagem de argumentos scrcpy: Linux (flags do cliente
//! `scrcpy`, verificadas contra `scrcpy --help` v4.0 real) e Windows
//! (argumentos de `com.genymobile.scrcpy.Server`, verificados contra
//! `scrcpy/app/src/server.c` upstream — research.md R12). Pura montagem de
//! `Vec<String>`, sem subprocess: roda em qualquer plataforma-host
//! (diferente de T018/T019, que dependem do host real). Escritos ANTES da
//! implementação (Princípio III); falham até T024 implementar
//! `src-tauri/src/stream_manager.rs`.

use camlink_lib::model::{StreamConfig, VideoCodec};
use camlink_lib::stream_manager::{
    build_scrcpy_client_args, build_server_launch_args, SCRCPY_DEVICE_SERVER_PATH, SCRCPY_VERSION,
};

fn config_h264() -> StreamConfig {
    StreamConfig {
        resolution: (1920, 1080),
        fps: 30,
        bitrate: 8_000_000,
        codec: VideoCodec::H264,
        camera_id: "0".into(),
    }
}

fn config_h265() -> StreamConfig {
    StreamConfig {
        resolution: (1280, 720),
        fps: 60,
        bitrate: 4_000_000,
        codec: VideoCodec::H265,
        camera_id: "1".into(),
    }
}

// ---------------------------------------------------------------------------
// build_scrcpy_client_args — Linux: cliente scrcpy stock com --v4l2-sink
// ---------------------------------------------------------------------------

#[test]
fn client_args_target_camera_source_and_v4l2_sink() {
    let args = build_scrcpy_client_args(&config_h264(), "/dev/video3", "R58M12ABCDE");
    assert!(args.contains(&"--video-source=camera".to_string()));
    assert!(args.contains(&"--v4l2-sink=/dev/video3".to_string()));
    assert!(args.contains(&"--camera-id=0".to_string()));
    assert!(args.contains(&"--camera-size=1920x1080".to_string()));
    assert!(args.contains(&"--camera-fps=30".to_string()));
    assert!(args.contains(&"--video-bit-rate=8000000".to_string()));
    assert!(args.contains(&"--video-codec=h264".to_string()));
}

#[test]
fn client_args_select_h265_codec() {
    let args = build_scrcpy_client_args(&config_h265(), "/dev/video0", "R58M12ABCDE");
    assert!(args.contains(&"--video-codec=h265".to_string()));
    assert!(args.contains(&"--camera-size=1280x720".to_string()));
    assert!(args.contains(&"--camera-fps=60".to_string()));
}

#[test]
fn client_args_are_headless_no_audio_no_control() {
    let args = build_scrcpy_client_args(&config_h264(), "/dev/video0", "R58M12ABCDE");
    assert!(args.contains(&"--no-window".to_string()));
    assert!(args.contains(&"--no-audio".to_string()));
    assert!(args.contains(&"--no-control".to_string()));
}

// Bug real em bancada 2026-08-08 (T065): com 2+ celulares Android plugados
// ao mesmo tempo, o cliente scrcpy sem `--serial` não sabe qual escolher
// ("Multiple ADB devices ... Select a device via -s") e a sessão entra num
// loop infinito de reconexão — nunca chega a rodar de verdade.
#[test]
fn client_args_include_serial_to_disambiguate_multiple_devices() {
    let args = build_scrcpy_client_args(&config_h264(), "/dev/video3", "R58M12ABCDE");
    assert!(args.contains(&"--serial=R58M12ABCDE".to_string()));
}

// ---------------------------------------------------------------------------
// build_server_launch_args — Windows: bootstrap direto do scrcpy-server
// (research.md R12), sem passar pelo cliente `scrcpy`
// ---------------------------------------------------------------------------

#[test]
fn server_args_start_with_adb_shell_app_process_invocation() {
    let args = build_server_launch_args(0x1234_abcd, &config_h264());
    let expected_prefix = vec![
        "shell".to_string(),
        format!("CLASSPATH={SCRCPY_DEVICE_SERVER_PATH}"),
        "app_process".to_string(),
        "/".to_string(),
        "com.genymobile.scrcpy.Server".to_string(),
        SCRCPY_VERSION.to_string(),
    ];
    assert_eq!(&args[..expected_prefix.len()], expected_prefix.as_slice());
}

#[test]
fn server_args_encode_scid_as_zero_padded_hex() {
    let args = build_server_launch_args(0x1234_abcd, &config_h264());
    assert!(
        args.contains(&"scid=1234abcd".to_string()),
        "args: {args:?}"
    );
}

#[test]
fn server_args_request_video_only_camera_source_over_forward_tunnel() {
    let args = build_server_launch_args(1, &config_h264());
    assert!(args.contains(&"video_source=camera".to_string()));
    assert!(args.contains(&"tunnel_forward=true".to_string()));
    assert!(args.contains(&"audio=false".to_string()));
    // US1/T024 usa o jar stock sem o socket de controle do fork (T037/US2).
    assert!(args.contains(&"control=false".to_string()));
}

#[test]
fn server_args_carry_stream_config_fields() {
    let args = build_server_launch_args(1, &config_h265());
    assert!(args.contains(&"camera_id=1".to_string()));
    assert!(args.contains(&"camera_size=1280x720".to_string()));
    assert!(args.contains(&"max_fps=60".to_string()));
    assert!(args.contains(&"video_bit_rate=4000000".to_string()));
}

#[test]
fn different_sessions_get_different_scids_in_args() {
    let a = build_server_launch_args(0x0000_0001, &config_h264());
    let b = build_server_launch_args(0x0000_0002, &config_h264());
    assert!(a.contains(&"scid=00000001".to_string()));
    assert!(b.contains(&"scid=00000002".to_string()));
}
