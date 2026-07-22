//! T048 — Testes de `secrets.rs` (US4 / FR-018a): credenciais RTSP vivem
//! SOMENTE no cofre do SO (keyring); a config persiste apenas a referência.
//!
//! Usa o credential store mock do crate `keyring` (em memória, por processo)
//! para não tocar o cofre real durante os testes — os testes rodam num único
//! processo de teste, então o builder default é configurado uma vez.

use std::sync::Once;

use camlink_lib::model::{AppConfig, RtspSource, RtspState};
use camlink_lib::secrets;
use uuid::Uuid;

static INIT: Once = Once::new();

fn use_mock_store() {
    INIT.call_once(|| {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    });
}

#[test]
fn store_retrieve_delete_roundtrip() {
    use_mock_store();
    let key = secrets::secret_ref_for(&Uuid::new_v4());

    secrets::store_secret(&key, "s3nh4-secreta").expect("store");
    let got = secrets::get_secret(&key).expect("get");
    assert_eq!(got.as_deref(), Some("s3nh4-secreta"));

    secrets::delete_secret(&key).expect("delete");
    let gone = secrets::get_secret(&key).expect("get após delete");
    assert_eq!(gone, None, "segredo deveria ter sido removido");
}

#[test]
fn get_missing_secret_is_none_not_error() {
    use_mock_store();
    let got = secrets::get_secret("camlink-rtsp-inexistente").expect("get");
    assert_eq!(got, None);
}

#[test]
fn store_overwrites_previous_value() {
    use_mock_store();
    let key = secrets::secret_ref_for(&Uuid::new_v4());
    secrets::store_secret(&key, "antiga").expect("store 1");
    secrets::store_secret(&key, "nova").expect("store 2");
    assert_eq!(
        secrets::get_secret(&key).expect("get").as_deref(),
        Some("nova")
    );
    secrets::delete_secret(&key).expect("cleanup");
}

#[test]
fn secret_ref_is_deterministic_and_namespaced() {
    let id = Uuid::new_v4();
    let a = secrets::secret_ref_for(&id);
    let b = secrets::secret_ref_for(&id);
    assert_eq!(a, b, "mesma fonte → mesma referência");
    assert!(a.contains(&id.to_string()), "referência deve conter o id");
    assert!(a.starts_with("camlink-"), "namespace do app: {a}");
}

/// FR-018a: a serialização da config (o que vai pro TOML em disco) nunca pode
/// conter a senha — só a URL sem credenciais e o `secret_ref`.
#[test]
fn serialized_config_never_contains_password() {
    let password = "senha-que-nao-pode-vazar";
    let source = RtspSource {
        id: Uuid::new_v4(),
        name: "Câmera do portão".into(),
        url: "rtsp://192.168.0.42:554/stream1".into(),
        secret_ref: Some(secrets::secret_ref_for(&Uuid::new_v4())),
        state: RtspState::Idle,
    };
    let config = AppConfig {
        rtsp_sources: vec![source],
        ..AppConfig::default()
    };

    let toml_text = toml::to_string(&config).expect("config → TOML");
    assert!(
        !toml_text.contains(password),
        "senha vazou para a config serializada"
    );
    let json_text = serde_json::to_string(&config).expect("config → JSON");
    assert!(!json_text.contains(password), "senha vazou para o JSON");
}
