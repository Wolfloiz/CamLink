//! T049 — Testes da pipeline RTSP (US4): montagem da linha de comando ffmpeg
//! low-delay (flags exatas de research.md R5), injeção de credencial SOMENTE
//! em runtime (a URL persistida nunca as contém — FR-018a), validação de URL
//! e classificação de falhas (erro de auth distinto de host inacessível).

use camlink_lib::rtsp_manager::{
    classify_ffmpeg_failure, ffmpeg_input_args, inject_credentials, output_args_rawvideo,
    output_args_v4l2, validate_rtsp_url, RtspFailure,
};

// ---------------------------------------------------------------------------
// Flags low-delay (R5)
// ---------------------------------------------------------------------------

#[test]
fn input_args_match_research_r5_low_delay_flags() {
    let args = ffmpeg_input_args("rtsp://192.168.0.42:554/stream1");
    assert_eq!(
        args,
        vec![
            "-fflags".to_string(),
            "nobuffer".to_string(),
            "-flags".to_string(),
            "low_delay".to_string(),
            "-analyzeduration".to_string(),
            "0".to_string(),
            "-probesize".to_string(),
            "32".to_string(),
            "-rtsp_transport".to_string(),
            "tcp".to_string(),
            "-i".to_string(),
            "rtsp://192.168.0.42:554/stream1".to_string(),
        ]
    );
}

#[test]
fn v4l2_output_targets_the_device() {
    let args = output_args_v4l2("/dev/video10");
    assert_eq!(
        args,
        vec![
            "-pix_fmt".to_string(),
            "yuyv422".to_string(),
            "-f".to_string(),
            "v4l2".to_string(),
            "/dev/video10".to_string(),
        ]
    );
}

#[test]
fn rawvideo_output_is_rgba_on_stdout() {
    let args = output_args_rawvideo();
    assert_eq!(
        args,
        vec![
            "-f".to_string(),
            "rawvideo".to_string(),
            "-pix_fmt".to_string(),
            "rgba".to_string(),
            "pipe:1".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// Credenciais em runtime (FR-018a)
// ---------------------------------------------------------------------------

#[test]
fn password_is_injected_after_existing_user() {
    let url = "rtsp://admin@192.168.0.42:554/stream1";
    let out = inject_credentials(url, Some("s3nh4"));
    assert_eq!(out, "rtsp://admin:s3nh4@192.168.0.42:554/stream1");
}

#[test]
fn user_and_password_pair_in_secret_is_injected_whole() {
    // Sem usuário na URL, o segredo pode carregar "user:senha" completo.
    let url = "rtsp://192.168.0.42:554/stream1";
    let out = inject_credentials(url, Some("admin:s3nh4"));
    assert_eq!(out, "rtsp://admin:s3nh4@192.168.0.42:554/stream1");
}

#[test]
fn no_secret_leaves_url_untouched() {
    let url = "rtsp://192.168.0.42:554/stream1";
    assert_eq!(inject_credentials(url, None), url);
}

#[test]
fn reserved_characters_in_password_are_percent_encoded() {
    // ffmpeg interpreta '@'/':'/'/' na URL; a senha precisa sair
    // percent-encoded para não quebrar o parsing nem vazar pro path.
    let url = "rtsp://admin@camera.local/live";
    let out = inject_credentials(url, Some("p@ss:w/rd"));
    assert_eq!(out, "rtsp://admin:p%40ss%3Aw%2Frd@camera.local/live");
}

// ---------------------------------------------------------------------------
// Validação de URL e classificação de falhas
// ---------------------------------------------------------------------------

#[test]
fn only_rtsp_schemes_are_accepted() {
    assert!(validate_rtsp_url("rtsp://camera.local/live").is_ok());
    assert!(validate_rtsp_url("rtsps://camera.local/live").is_ok());
    assert!(validate_rtsp_url("http://camera.local/live").is_err());
    assert!(validate_rtsp_url("file:///etc/passwd").is_err());
    assert!(validate_rtsp_url("rtsp://").is_err(), "URL sem host");
}

#[test]
fn auth_failure_is_distinguished_from_unreachable_host() {
    let auth = classify_ffmpeg_failure(
        "[rtsp @ 0x55] method DESCRIBE failed: 401 Unauthorized\nrtsp://cam/live: Server returned 401 Unauthorized (authorization failed)",
    );
    assert_eq!(auth, RtspFailure::AuthFailed);

    let unreachable = classify_ffmpeg_failure("rtsp://10.0.0.9/live: Connection refused");
    assert_eq!(unreachable, RtspFailure::Unreachable);

    let timeout = classify_ffmpeg_failure("rtsp://10.0.0.9/live: Connection timed out");
    assert_eq!(timeout, RtspFailure::Unreachable);

    let other = classify_ffmpeg_failure("algum stderr aleatório do ffmpeg");
    assert!(matches!(other, RtspFailure::Other(_)));
}

#[test]
fn failure_messages_are_actionable() {
    // FR-010: erro com dica de ação, não só o código.
    let auth = classify_ffmpeg_failure("401 Unauthorized");
    let msg = auth.action_hint();
    assert!(
        msg.to_lowercase().contains("senha") || msg.to_lowercase().contains("credencia"),
        "dica de auth deveria mencionar credenciais: {msg}"
    );

    let unreachable = classify_ffmpeg_failure("Connection refused");
    let msg = unreachable.action_hint();
    assert!(
        msg.to_lowercase().contains("rede")
            || msg.to_lowercase().contains("endereço")
            || msg.to_lowercase().contains("host"),
        "dica de rede deveria orientar sobre endereço/host: {msg}"
    );
}
