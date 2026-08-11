//! Binário fake usado por `stream_lifecycle_test.rs` (T018) no lugar do
//! cliente scrcpy (Linux) ou do bootstrap adb (Windows) — research.md R11.
//!
//! Comportamento controlado por variáveis de ambiente (nunca por argv, já
//! que `stream_manager.rs` invoca este binário com os mesmos argumentos que
//! usaria para o adb/scrcpy reais):
//!
//! - `FAKE_BACKEND_MODE`: `stay_alive` (default) | `crash_once` | `stderr_error`
//! - `FAKE_BACKEND_MARKER_FILE`: usado só em `crash_once`, para diferenciar a
//!   primeira tentativa (crasha) de uma retentativa (fica de pé) — simula
//!   "crash → Reconnecting → retomada" de forma determinística.
//! - `FAKE_BACKEND_STDERR_LINE`: usado só em `stderr_error`, texto a emitir.
//!
//! Chamadas de setup curtas (`push`/`forward`, usadas só no bootstrap
//! Windows de research.md R12; `pkill`/`pgrep`, usadas no Linux por
//! `spawn_backend` para matar um `scrcpy-server` remanescente ANTES de subir
//! um novo — ver doc de `spawn_backend` em `stream_manager.rs`) sempre
//! retornam sucesso imediato, independente do modo, para não interferir na
//! fase de setup nem cair no `stay_alive()` (3600s) por engano.

use std::env;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args
        .iter()
        .any(|a| a == "push" || a == "forward" || a == "pkill" || a == "pgrep")
    {
        return ExitCode::SUCCESS;
    }

    let mode = env::var("FAKE_BACKEND_MODE").unwrap_or_else(|_| "stay_alive".to_string());
    match mode.as_str() {
        "crash_once" => {
            let marker = env::var("FAKE_BACKEND_MARKER_FILE")
                .expect("FAKE_BACKEND_MARKER_FILE obrigatório em crash_once");
            let delay_ms: u64 = env::var("FAKE_BACKEND_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50);
            if !Path::new(&marker).exists() {
                let _ = std::fs::write(&marker, b"attempted");
                thread::sleep(Duration::from_millis(delay_ms));
                return ExitCode::FAILURE;
            }
            stay_alive();
            ExitCode::SUCCESS
        }
        "stderr_error" => {
            let line = env::var("FAKE_BACKEND_STDERR_LINE")
                .unwrap_or_else(|_| "error: device unauthorized".to_string());
            eprintln!("{line}");
            ExitCode::FAILURE
        }
        _ => {
            stay_alive();
            ExitCode::SUCCESS
        }
    }
}

fn stay_alive() {
    thread::sleep(Duration::from_secs(3600));
}
