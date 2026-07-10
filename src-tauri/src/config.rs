//! Persistência de `AppConfig` em TOML no diretório de configuração da
//! plataforma; nunca contém credenciais (FR-018a).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::AppConfig;

/// Falha de carga/gravação da configuração. Mensagens citam o caminho para
/// serem acionáveis (FR-010).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("falha de E/S no arquivo de configuração {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("configuração inválida em {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("falha ao serializar a configuração: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Carrega a configuração de `path`; arquivo ausente é erro (use
/// [`load_or_default`] para tolerar primeira execução).
pub fn load_from(path: &Path) -> Result<AppConfig, ConfigError> {
    let content = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let config = toml::from_str(&content).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    tracing::debug!(path = %path.display(), "configuração carregada");
    Ok(config)
}

/// Carrega a configuração de `path`; se o arquivo não existe (primeira
/// execução), devolve `AppConfig::default()`. Arquivo corrompido continua
/// sendo erro — nunca sobrescrevemos silenciosamente uma config ilegível.
pub fn load_or_default(path: &Path) -> Result<AppConfig, ConfigError> {
    match load_from(path) {
        Ok(config) => Ok(config),
        Err(ConfigError::Io { ref source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            tracing::info!(path = %path.display(), "configuração ausente; usando defaults");
            Ok(AppConfig::default())
        }
        Err(err) => Err(err),
    }
}

/// Grava a configuração em `path` como TOML, criando os diretórios pai se
/// necessário. O conteúdo nunca inclui credenciais (FR-018a) — `AppConfig`
/// só carrega `secret_ref`, a referência ao cofre do SO.
pub fn save_to(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let content = toml::to_string_pretty(config)?;
    fs::write(path, content).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    tracing::debug!(path = %path.display(), "configuração gravada");
    Ok(())
}

/// Caminho padrão do arquivo de configuração no diretório de config da
/// plataforma (Linux: `~/.config/camlink/config.toml`; Windows:
/// `%APPDATA%\camlink\config.toml`).
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("camlink").join("config.toml"))
}
