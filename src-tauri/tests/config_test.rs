//! T009 — Testes de persistência de `AppConfig` em TOML (data-model.md).
//!
//! Regra central: o arquivo de configuração NUNCA contém credenciais
//! (FR-018a) — senhas vivem apenas no cofre do SO, referenciadas por
//! `secret_ref`. Escritos ANTES da implementação (Princípio III); falham
//! até T013 implementar `src-tauri/src/config.rs`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use camlink_lib::config::{load_from, load_or_default, save_to};
use camlink_lib::model::{AppConfig, RtspSource, RtspState, StreamConfig, VideoCodec};
use uuid::Uuid;

fn sample_config() -> AppConfig {
    let mut last_stream_config = HashMap::new();
    last_stream_config.insert(
        "R58M12ABCDE".to_string(),
        StreamConfig {
            resolution: (1920, 1080),
            fps: 30,
            bitrate: 8_000_000,
            codec: VideoCodec::H264,
            camera_id: "0".into(),
        },
    );
    AppConfig {
        rtsp_sources: vec![RtspSource {
            id: Uuid::new_v4(),
            name: "Portaria".into(),
            url: "rtsp://10.0.0.9:554/live".into(),
            secret_ref: Some("camlink/rtsp/portaria".into()),
            state: RtspState::Idle,
        }],
        last_stream_config,
        raw_output_dir: Some(PathBuf::from("/home/user/Imagens/CamLink")),
        auto_connect: vec!["R58M12ABCDE".into()],
        device_nicknames: HashMap::new(),
    }
}

#[test]
fn save_then_load_roundtrips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let config = sample_config();

    save_to(&path, &config).expect("save_to falhou");
    let loaded = load_from(&path).expect("load_from falhou");
    assert_eq!(loaded, config);
}

#[test]
fn saved_file_is_valid_toml_without_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    save_to(&path, &sample_config()).expect("save_to falhou");

    let content = fs::read_to_string(&path).expect("arquivo de config ausente");

    // FR-018a: nenhum campo de senha/credencial no arquivo; apenas a
    // referência ao cofre do SO.
    let lower = content.to_lowercase();
    assert!(
        !lower.contains("password"),
        "config contém 'password':\n{content}"
    );
    assert!(
        !lower.contains("senha"),
        "config contém 'senha':\n{content}"
    );
    assert!(
        content.contains("camlink/rtsp/portaria"),
        "secret_ref (referência ao cofre) deve ser persistida:\n{content}"
    );
    // URL persistida sem credenciais embutidas (user:pass@host).
    assert!(
        !content.contains('@'),
        "URL com credencial embutida:\n{content}"
    );
}

#[test]
fn save_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join("subdir")
        .join("aninhado")
        .join("config.toml");

    save_to(&path, &AppConfig::default()).expect("save_to deve criar diretórios pai");
    assert!(path.exists());
}

#[test]
fn load_from_missing_file_is_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nao_existe.toml");
    assert!(load_from(&path).is_err());
}

#[test]
fn load_or_default_returns_default_when_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nao_existe.toml");

    let config = load_or_default(&path).expect("load_or_default falhou");
    assert_eq!(config, AppConfig::default());
}

#[test]
fn app_config_default_is_empty() {
    let config = AppConfig::default();
    assert!(config.rtsp_sources.is_empty());
    assert!(config.last_stream_config.is_empty());
    assert!(config.raw_output_dir.is_none());
    assert!(config.auto_connect.is_empty());
}

#[test]
fn load_from_corrupt_toml_is_descriptive_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(&path, "isto = [não é TOML válido").expect("write");

    let err = load_from(&path).expect_err("TOML corrompido deve falhar sem panic");
    assert!(!err.to_string().is_empty(), "erro deve ter mensagem");
}

#[test]
fn save_overwrites_existing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    save_to(&path, &sample_config()).expect("primeiro save");
    let novo = AppConfig::default();
    save_to(&path, &novo).expect("segundo save");

    let loaded = load_from(&path).expect("load após overwrite");
    assert_eq!(loaded, novo);
}
