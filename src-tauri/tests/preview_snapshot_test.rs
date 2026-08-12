//! Timeout do snapshot de preview (Linux) — regressão do bug achado no
//! diagnóstico D1 (2026-08-11): `run_preview_pipeline` fazia
//! `stdout.read_exact(&mut buf).await` sem timeout nenhum, então um ffmpeg
//! que abre o device v4l2 mas nunca recebe frame penduraava a task PRA
//! SEMPRE — o preview daquela sessão morria em definitivo e o processo
//! seguia segurando o device (o que ainda fazia o `purge_all` do T065d
//! falhar com EBUSY, deixando device fantasma no OBS). Evidência direta:
//! um `ffmpeg -frames:v 1` órfão encontrado travado há 1h55m em
//! `/dev/video8`, sem writer nenhum no device.
//!
//! Escrito ANTES da implementação (Princípio III). Este arquivo só compila
//! no Linux (o pipeline de preview é `#[cfg(target_os = "linux")]`).

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use camlink_lib::stream_manager::capture_preview_snapshot;

fn fake_backend_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_backend"))
}

/// Env por chamada (nunca `std::env::set_var`): os dois testes rodam em
/// paralelo no MESMO processo, então env global vira corrida — um teste
/// sobrescreve o marker do outro e o PID chega vazio.
fn hang_env(marker: &std::path::Path) -> Vec<(String, String)> {
    vec![
        ("FAKE_BACKEND_MODE".into(), "hang".into()),
        (
            "FAKE_BACKEND_MARKER_FILE".into(),
            marker.to_string_lossy().into_owned(),
        ),
    ]
}

/// O fake em `hang` nunca escreve nada no stdout e nunca sai — exatamente o
/// ffmpeg travado do achado. Sem timeout, esta chamada não retorna nunca (o
/// teste inteiro pendura); com timeout, devolve `Err` rápido.
#[tokio::test]
async fn snapshot_desiste_no_timeout_em_vez_de_pendurar_pra_sempre() {
    let marker = tempfile::NamedTempFile::new().expect("tempfile");
    let mut buf = vec![0u8; 640 * 360 * 4];
    let started = Instant::now();
    let result = capture_preview_snapshot(
        &fake_backend_path(),
        &hang_env(marker.path()),
        "/dev/video99",
        (1920, 1080),
        &mut buf,
        Duration::from_millis(300),
    )
    .await;
    let elapsed = started.elapsed();

    assert!(
        result.is_err(),
        "snapshot que nunca recebe frame tem que virar erro, não sucesso"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "deveria desistir em ~300ms, mas levou {elapsed:?} — timeout não está sendo aplicado"
    );
}

/// Desistir não basta: o processo precisa MORRER, senão ele continua
/// segurando o slot único de leitura do device (o que bloqueia o consumidor
/// real e faz o `v4l2loopback-ctl delete` falhar com EBUSY). Vale lembrar
/// que um ffmpeg bloqueado no v4l2 IGNORA SIGTERM — reproduzido 3x no D1,
/// só morre com SIGKILL, que é o que o `kill_on_drop` do tokio manda.
#[tokio::test]
async fn snapshot_mata_o_processo_ao_desistir() {
    let marker = tempfile::NamedTempFile::new().expect("tempfile");
    let mut buf = vec![0u8; 640 * 360 * 4];
    let _ = capture_preview_snapshot(
        &fake_backend_path(),
        &hang_env(marker.path()),
        "/dev/video99",
        (1920, 1080),
        &mut buf,
        Duration::from_millis(300),
    )
    .await;

    let pid: i32 = std::fs::read_to_string(marker.path())
        .expect("fake deveria ter gravado o PID")
        .trim()
        .parse()
        .expect("PID numérico");

    // O kill é assíncrono (tokio reaps em background) — dá uma janela curta.
    let mut alive = true;
    for _ in 0..40 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !alive,
        "o ffmpeg do snapshot ficou vivo (PID {pid}) depois do timeout — \
         é exatamente assim que o device fica preso e vira câmera fantasma"
    );
}
