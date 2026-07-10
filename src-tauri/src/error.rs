//! Erro central da aplicação: `AppError { code, msg, action_hint }`
//! integrado ao `tracing` (Princípio VI, FR-010).

use serde::{Deserialize, Serialize};

/// Erro exposto ao frontend via IPC (contracts/tauri-commands.md): código
/// estável para tratamento programático, mensagem acionável para o usuário e
/// dica opcional do próximo passo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("[{code}] {msg}")]
pub struct AppError {
    pub code: String,
    pub msg: String,
    pub action_hint: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            msg: msg.into(),
            action_hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.action_hint = Some(hint.into());
        self
    }

    /// Registra o erro no `tracing` estruturado e o devolve, para uso em
    /// cadeia: `return Err(AppError::new(...).trace())`.
    pub fn trace(self) -> Self {
        tracing::error!(
            code = %self.code,
            action_hint = self.action_hint.as_deref(),
            "{}",
            self.msg
        );
        self
    }
}

impl From<crate::model::InvalidTransition> for AppError {
    fn from(err: crate::model::InvalidTransition) -> Self {
        AppError::new("invalid_transition", err.to_string())
    }
}
