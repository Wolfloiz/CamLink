//! T102 — nível do log do probe RTSP.
//!
//! `probe_url_with_retry` existe porque a 1ª tentativa falhar é NORMAL numa
//! câmera recém-ligada (T054). Logar cada uma delas em WARN transforma um
//! início bem-sucedido em três linhas de alarme — exatamente o erro que o
//! T099 corrigiu no pipeline Android, onde três cliques normais do usuário
//! mandaram o autor caçar um defeito que não existia.
//!
//! Este teste mora num binário SÓ DELE de propósito. O `tracing` decide uma
//! única vez, por callsite, se alguém se interessa pelo evento, e guarda a
//! resposta num cache global do processo: se um teste vizinho executar
//! estes mesmos `warn!` primeiro, sem assinante, o cache fica em "ninguém
//! quer" e a captura chega vazia — o teste falharia por ordem de execução,
//! não por regressão. Rodando sozinho, o assinante está instalado antes do
//! primeiro evento. Pela mesma razão, não acrescente outros testes aqui.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camlink_lib::rtsp_manager::probe_url_with_retry;

#[derive(Clone, Default)]
struct CapturaDeLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturaDeLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut dentro) = self.0.lock() {
            dentro.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturaDeLog {
    type Writer = CapturaDeLog;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn so_a_ultima_tentativa_do_probe_vira_warn() {
    let marker = tempfile::NamedTempFile::new().expect("tempfile");
    let captura = CapturaDeLog::default();
    let assinante = tracing_subscriber::fmt()
        .with_writer(captura.clone())
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .finish();
    let guarda = tracing::subscriber::set_default(assinante);

    // `fail_then_succeed` com 99 falhas: nenhuma tentativa suceder é o
    // cenário em que o usuário merece UM alarme, não três.
    let env = vec![
        (
            "FAKE_BACKEND_MODE".to_string(),
            "fail_then_succeed".to_string(),
        ),
        (
            "FAKE_BACKEND_MARKER_FILE".to_string(),
            marker.path().to_string_lossy().into_owned(),
        ),
        ("FAKE_BACKEND_FAIL_COUNT".to_string(), "99".to_string()),
    ];
    let result = probe_url_with_retry(
        &PathBuf::from(env!("CARGO_BIN_EXE_fake_backend")),
        "rtsp://127.0.0.1:8554/teste",
        &env,
        3,
        Duration::from_millis(10),
    )
    .await;
    drop(guarda);
    assert!(result.is_err(), "a fonte nunca fica pronta neste cenário");

    let texto = String::from_utf8(captura.0.lock().expect("lock").clone()).expect("utf8");
    let mencoes = texto
        .lines()
        .filter(|l| l.contains("probe RTSP falhou"))
        .count();
    let warns = texto
        .lines()
        .filter(|l| l.contains("WARN") && l.contains("probe RTSP falhou"))
        .count();
    assert_eq!(
        mencoes, 3,
        "as três tentativas precisam aparecer em ALGUM nível: {texto}"
    );
    assert_eq!(
        warns, 1,
        "só a tentativa que desiste é um problema do usuário; as de \
         aquecimento são esperadas: {texto}"
    );
    assert!(
        texto.contains("stderr="),
        "o WARN precisa carregar o stderr do ffmpeg — sem ele o log volta a \
         dizer só o código e nada da causa (T101): {texto}"
    );
}
