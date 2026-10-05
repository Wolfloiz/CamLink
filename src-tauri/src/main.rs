// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Quantos arquivos diários de log manter. Sete dias cobre "aconteceu na
/// semana passada" sem deixar o diretório crescer sem limite.
const LOG_FILES_KEPT: usize = 7;

fn main() {
    // `_log_guard` TEM que viver até o fim do main: ele é o handle da thread
    // que escreve o arquivo. Dropá-lo mais cedo descarta o que ainda não foi
    // gravado — justamente as últimas linhas, que são as interessantes
    // quando o app morre.
    let (_log_guard, log_path) = init_tracing();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "iniciando CamLink");
    match &log_path {
        Some(path) => tracing::info!(arquivo = %path.display(), "log em arquivo"),
        None => tracing::warn!("sem log em arquivo (não foi possível criar o diretório)"),
    }

    camlink_lib::run()
}

/// Liga o `tracing` no stdout E num arquivo.
///
/// O arquivo existe por um motivo específico: no Windows o build de release
/// roda com `windows_subsystem = "windows"`, ou seja SEM console, e um
/// `tracing_subscriber::fmt()` para stdout escreve no vazio. Resultado: no
/// Linux um problema instalado é diagnosticável em minutos pelo terminal, e
/// no Windows não havia como obter NADA — o usuário só tinha a mensagem de
/// erro da interface, e cada hipótese custava um ciclo inteiro de release
/// (bancada 2026-10-05, com o app falhando só no Windows).
///
/// O log é local e não sai da máquina: nada aqui envia nada para lugar
/// nenhum (Princípio de privacidade), e as senhas de fontes RTSP continuam
/// censuradas na origem, antes de chegar ao `tracing`.
fn init_tracing() -> (Option<WorkerGuard>, Option<PathBuf>) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());

    let Some(dir) = log_dir() else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer())
            .init();
        return (None, None);
    };

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("camlink")
        .filename_suffix("log")
        .max_log_files(LOG_FILES_KEPT)
        .build(&dir);

    match appender {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::registry()
                .with(filter)
                // Sem ANSI no arquivo: códigos de cor viram lixo num editor
                // e atrapalham quem for ler ou colar o log.
                .with(fmt::layer().with_ansi(false).with_writer(writer))
                .with(fmt::layer())
                .init();
            (Some(guard), Some(dir))
        }
        // Falha ao abrir o arquivo (disco cheio, permissão) nunca impede o
        // app de subir: perder o log é ruim, não funcionar é pior.
        Err(_) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer())
                .init();
            (None, None)
        }
    }
}

/// `~/.local/share/CamLink/logs` no Linux, `%LOCALAPPDATA%\CamLink\logs` no
/// Windows — o mesmo `data_local_dir` que o resto do app usa para estado
/// local, criado sob demanda.
fn log_dir() -> Option<PathBuf> {
    let dir = dirs::data_local_dir()?
        .join(camlink_lib::paths::PRODUCT_NAME)
        .join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
