//! Binário fake usado por `stream_lifecycle_test.rs` (T018) no lugar do
//! cliente scrcpy (Linux) ou do bootstrap adb (Windows) — research.md R11.
//!
//! Comportamento controlado por variáveis de ambiente (nunca por argv, já
//! que `stream_manager.rs` invoca este binário com os mesmos argumentos que
//! usaria para o adb/scrcpy reais):
//!
//! - `FAKE_BACKEND_MODE`: `stay_alive` (default) | `crash_once` | `stderr_error`
//!   | `fail_then_succeed` | `hang`
//! - `FAKE_BACKEND_MARKER_FILE`: usado em `crash_once` e `fail_then_succeed`,
//!   para diferenciar a primeira tentativa de uma retentativa — simula
//!   "crash → Reconnecting → retomada" (`crash_once`, fica de pé depois) ou
//!   uma fonte RTSP que só fica pronta depois de N tentativas
//!   (`fail_then_succeed`, sai rápido em vez de ficar de pé — usado em
//!   `probe_url_with_retry`, que espera o processo TERMINAR, não continuar
//!   rodando). Em `hang`, recebe o PID do próprio fake.
//! - `FAKE_BACKEND_STDERR_LINE`: usado só em `stderr_error`, texto a emitir.
//! - `FAKE_BACKEND_FAIL_COUNT`: usado só em `fail_then_succeed`, quantas
//!   tentativas falham antes da que sai com sucesso (default 1).
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
        // Trava pra sempre sem produzir nada no stdout — reproduz o ffmpeg de
        // preview que abre o device v4l2 mas nunca recebe frame (achado no D1,
        // 2026-08-11: um órfão desses ficou 1h55m segurando /dev/video8).
        // Grava o próprio PID no marker pra o teste conseguir provar que o
        // processo foi MORTO no timeout, não só abandonado.
        "hang" => {
            if let Ok(marker) = env::var("FAKE_BACKEND_MARKER_FILE") {
                let _ = std::fs::write(&marker, std::process::id().to_string());
            }
            stay_alive();
            ExitCode::SUCCESS
        }
        "fail_then_succeed" => {
            let marker = env::var("FAKE_BACKEND_MARKER_FILE")
                .expect("FAKE_BACKEND_MARKER_FILE obrigatório em fail_then_succeed");
            let fail_count: u32 = env::var("FAKE_BACKEND_FAIL_COUNT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let attempts: u32 = std::fs::read_to_string(&marker)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let _ = std::fs::write(&marker, (attempts + 1).to_string());
            if attempts < fail_count {
                eprintln!("fake source ainda não está pronta");
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
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
