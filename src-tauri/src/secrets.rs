//! T050 — Cofre de segredos (US4 / FR-018a): credenciais RTSP vivem SOMENTE
//! no cofre do SO via crate `keyring` (Secret Service no Linux, Credential
//! Manager no Windows). A config persiste apenas `secret_ref`; nada em claro.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use uuid::Uuid;

/// Namespace de serviço no cofre — agrupa as entradas do CamLink.
const SERVICE: &str = "camlink";

/// Cache de handles por referência. Necessário para o credential store mock
/// dos testes (cada `Entry::new` mock é independente — estado só persiste
/// reutilizando o mesmo `Entry`); no cofre real evita recriar handles.
fn entry_for(secret_ref: &str) -> Result<Arc<keyring::Entry>, SecretError> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<keyring::Entry>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().expect("cache de entries envenenado");
    if let Some(entry) = map.get(secret_ref) {
        return Ok(Arc::clone(entry));
    }
    let entry = Arc::new(keyring::Entry::new(SERVICE, secret_ref)?);
    map.insert(secret_ref.to_string(), Arc::clone(&entry));
    Ok(entry)
}

/// Falha de acesso ao cofre do SO, com dica acionável (FR-010).
#[derive(Debug, thiserror::Error)]
#[error("cofre de segredos do sistema indisponível: {source}")]
pub struct SecretError {
    #[from]
    source: keyring::Error,
}

/// Referência determinística de segredo para uma fonte RTSP — é isso (e só
/// isso) que a config em disco pode conter.
pub fn secret_ref_for(source_id: &Uuid) -> String {
    format!("camlink-rtsp-{source_id}")
}

/// Grava (ou sobrescreve) um segredo no cofre do SO.
pub fn store_secret(secret_ref: &str, secret: &str) -> Result<(), SecretError> {
    entry_for(secret_ref)?.set_password(secret)?;
    Ok(())
}

/// Lê um segredo; `None` quando não existe (não é erro — a fonte pode nunca
/// ter tido senha).
pub fn get_secret(secret_ref: &str) -> Result<Option<String>, SecretError> {
    match entry_for(secret_ref)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Remove um segredo; idempotente (remover o que não existe não é erro —
/// `remove_rtsp_source` precisa poder limpar sem checar antes).
pub fn delete_secret(secret_ref: &str) -> Result<(), SecretError> {
    match entry_for(secret_ref)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
