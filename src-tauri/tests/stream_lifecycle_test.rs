//! T018 — Testes de lifecycle de `StreamManager` com um binário fake no
//! lugar do backend real (cliente scrcpy no Linux; bootstrap adb no Windows
//! — research.md R12). Cobre start/stop, crash→Reconnecting→retomada
//! (FR-005/006) e stderr → erro acionável (FR-010). Escritos ANTES da
//! implementação (Princípio III); falham até T024 implementar
//! `src-tauri/src/stream_manager.rs`.

use std::path::PathBuf;
use std::time::Duration;

use camlink_lib::model::{SessionSource, SessionState, StreamConfig, VideoCodec};
use camlink_lib::stream_manager::{
    classify_stderr, require_server_jar, ExternalPaths, StreamManager, SCRCPY_VERSION,
};

fn fake_backend_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_backend"))
}

fn base_paths(extra_env: Vec<(String, String)>) -> ExternalPaths {
    let fake = fake_backend_path();
    ExternalPaths {
        adb: fake.clone(),
        scrcpy: fake,
        ffmpeg: PathBuf::from("ffmpeg"),
        // Precisa ser um arquivo que EXISTE: `require_server_jar` valida
        // isso antes de spawnar, porque um jar configurado mas ausente
        // (pacote incompleto, SCRCPY_SERVER_PATH obsoleto) terminava em 6
        // reconexões culpando o cabo USB. O fake_backend serve de stand-in:
        // nada aqui o lê como jar de verdade.
        server_jar: Some(fake_backend_path()),
        extra_env,
    }
}

fn sample_config() -> StreamConfig {
    StreamConfig {
        resolution: (1920, 1080),
        fps: 30,
        bitrate: 8_000_000,
        codec: VideoCodec::H264,
        camera_id: "0".into(),
    }
}

/// Espera (com timeout) até que `state()` satisfaça `pred`, para não
/// depender de sleeps fixos numa máquina de estados dirigida por subprocesso
/// real.
async fn wait_for_state<F>(
    manager: &StreamManager,
    session_id: uuid::Uuid,
    timeout: Duration,
    pred: F,
) -> SessionState
where
    F: Fn(&SessionState) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(state) = manager.state(session_id).await {
            if pred(&state) {
                return state;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timeout esperando estado esperado; último estado: {:?}",
                manager.state(session_id).await
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn start_reaches_streaming_then_stop_reaches_idle() {
    let manager = StreamManager::new(base_paths(vec![(
        "FAKE_BACKEND_MODE".into(),
        "stay_alive".into(),
    )]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder com backend fake saudável");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        *s == SessionState::Streaming
    })
    .await;

    manager.stop(session_id).await.expect("stop deve suceder");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        *s == SessionState::Idle
    })
    .await;
}

#[tokio::test]
async fn backend_crash_triggers_reconnect_and_recovers_streaming() {
    let marker = tempfile::NamedTempFile::new()
        .expect("tempfile")
        .path()
        .to_path_buf();
    // O arquivo não deve existir ainda: a primeira execução do fake cria o
    // marcador e crasha; a segunda (retomada) o encontra e fica de pé.
    let _ = std::fs::remove_file(&marker);

    let manager = StreamManager::new(base_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "crash_once".into()),
        (
            "FAKE_BACKEND_MARKER_FILE".into(),
            marker.to_string_lossy().into_owned(),
        ),
        ("FAKE_BACKEND_DELAY_MS".into(), "50".into()),
    ]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder mesmo que o backend vá crashar em seguida");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Reconnecting | SessionState::SourceLost)
    })
    .await;

    let final_state = wait_for_state(&manager, session_id, Duration::from_secs(5), |s| {
        *s == SessionState::Streaming
    })
    .await;
    assert_eq!(final_state, SessionState::Streaming);
}

#[tokio::test]
async fn stop_while_reconnecting_reaches_idle() {
    // Antes da correção de SessionControl, `stop()` só sabia mandar o
    // evento `Stop` (válido apenas a partir de `Streaming` na máquina de
    // estados) e só olhava `RunningSession.child` diretamente — que fica
    // `None` a maior parte do tempo (o processo real fica "emprestado"
    // dentro da task de monitor). Parar durante `Reconnecting` batia nos
    // dois problemas ao mesmo tempo: falhava a transição E não achava
    // processo nenhum para matar.
    let marker = tempfile::NamedTempFile::new()
        .expect("tempfile")
        .path()
        .to_path_buf();
    let _ = std::fs::remove_file(&marker);

    let manager = StreamManager::new(base_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "crash_once".into()),
        (
            "FAKE_BACKEND_MARKER_FILE".into(),
            marker.to_string_lossy().into_owned(),
        ),
        ("FAKE_BACKEND_DELAY_MS".into(), "50".into()),
    ]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder mesmo que o backend vá crashar em seguida");

    wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Reconnecting | SessionState::SourceLost)
    })
    .await;

    manager
        .stop(session_id)
        .await
        .expect("stop deve suceder mesmo durante reconexão");

    assert_eq!(manager.state(session_id).await, Some(SessionState::Idle));
}

#[tokio::test]
async fn known_fatal_stderr_maps_to_actionable_error_without_retry() {
    let manager = StreamManager::new(base_paths(vec![
        ("FAKE_BACKEND_MODE".into(), "stderr_error".into()),
        (
            "FAKE_BACKEND_STDERR_LINE".into(),
            "error: device unauthorized".into(),
        ),
    ]));

    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start deve suceder; o erro chega via stderr assíncrono");

    let state = wait_for_state(&manager, session_id, Duration::from_secs(3), |s| {
        matches!(s, SessionState::Error(_))
    })
    .await;
    match state {
        SessionState::Error(msg) => assert!(
            !msg.is_empty(),
            "estado Error deve carregar mensagem acionável"
        ),
        other => panic!("esperava Error, obteve {other:?}"),
    }

    // Erro fatal não deve entrar em retry automático: o estado permanece
    // Error mesmo após esperar além do tempo de uma reconexão normal.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(matches!(
        manager.state(session_id).await,
        Some(SessionState::Error(_))
    ));
}

#[tokio::test]
async fn stop_on_unknown_session_is_actionable_error() {
    let manager = StreamManager::new(base_paths(vec![]));
    let err = manager
        .stop(uuid::Uuid::new_v4())
        .await
        .expect_err("stop de sessão inexistente deve falhar");
    assert!(!err.msg.is_empty());
}

// ---------------------------------------------------------------------------
// classify_stderr — mapeamento de linhas conhecidas do adb/scrcpy para
// AppError acionável (FR-010); erros desconhecidos ficam None (tratados como
// perda transitória de sinal pelo monitor de lifecycle).
// ---------------------------------------------------------------------------

#[test]
fn classify_stderr_recognizes_unauthorized() {
    let err = classify_stderr("error: device unauthorized").expect("deve classificar");
    assert_eq!(err.code, "device_unauthorized");
    assert!(err.action_hint.is_some());
}

#[test]
fn classify_stderr_recognizes_device_not_found() {
    let err = classify_stderr("adb: no devices/emulators found").expect("deve classificar");
    assert_eq!(err.code, "device_not_found");
}

#[test]
fn classify_stderr_recognizes_version_mismatch() {
    let err =
        classify_stderr("adb server version doesn't match this client").expect("deve classificar");
    assert_eq!(err.code, "scrcpy_version_mismatch");
}

#[test]
fn classify_stderr_ignores_unknown_lines() {
    assert!(classify_stderr("INFO: Texture: 1920x1080").is_none());
    assert!(classify_stderr("").is_none());
}

// ---------------------------------------------------------------------------
// SCRCPY_SERVER_PATH precisa CHEGAR ao cliente scrcpy (Linux)
//
// `resolve_external_paths()` resolve o jar do fork, mas até esta correção o
// caminho morria ali: era só lido para preencher `server_jar`, nunca
// repassado ao processo filho. O scrcpy então enviava o server DELE ao
// celular em vez do nosso fork, e sem o fork não existe o socket
// `localabstract:camlink` — todo controle de câmera falhava com "conexão de
// controle encerrada pelo servidor".
//
// Passou despercebido porque em desenvolvimento a variável costuma estar
// exportada no shell e o filho a herda. Num pacote instalado não há shell.
// Relatado em bancada com o AppImage (2026-10-03).
// ---------------------------------------------------------------------------

/// T094 (bancada 2026-10-04, `cargo tauri dev`): sem jar do fork resolvido,
/// a sessão só morria e o supervisor reconectava 6 vezes, terminando em
/// "desconecte e reconecte o cabo USB" — culpando o cabo por um jar que
/// nunca existiu. A causa era o `PathBuf::from("scrcpy-server")` de
/// placeholder, que o T091 tornou autoritativo ao exportá-lo para o filho.
#[tokio::test]
async fn sem_jar_do_fork_o_erro_e_acionavel_e_nao_reconexao() {
    let mut paths = base_paths(Vec::new());
    paths.server_jar = None;

    let err =
        require_server_jar(&paths).expect_err("sem jar resolvido tem que ser erro, não tentativa");

    assert_eq!(err.code, "scrcpy_server_jar_ausente");
    assert!(
        err.action_hint.is_some(),
        "o erro precisa dizer o que fazer, não só que falhou"
    );
}

/// Jar configurado mas ausente em disco (pacote incompleto,
/// `SCRCPY_SERVER_PATH` apontando pra um build apagado) cai no mesmo erro
/// acionável — era o outro caminho para as 6 reconexões.
#[tokio::test]
async fn jar_do_fork_configurado_mas_ausente_tambem_e_acionavel() {
    let mut paths = base_paths(Vec::new());
    paths.server_jar = Some(PathBuf::from("/tmp/camlink-jar-que-nao-existe"));

    let err = require_server_jar(&paths).expect_err("jar ausente em disco tem que ser erro");

    assert_eq!(err.code, "scrcpy_server_jar_ausente");
    assert!(
        err.msg.contains("camlink-jar-que-nao-existe"),
        "a mensagem deve dizer QUAL caminho faltou, senão não ajuda a corrigir: {}",
        err.msg
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn scrcpy_client_receives_the_fork_jar_path() {
    let dump = tempfile::NamedTempFile::new().expect("temp");
    let dump_path = dump.path().to_path_buf();

    // Arquivo real, e não um caminho simbólico tipo
    // `/usr/lib/CamLink/bin/scrcpy-server-camlink`: aquele não existe na
    // máquina de quem roda os testes, e o teste só passava porque nada
    // checava existência. `require_server_jar` agora checa.
    let jar_file = tempfile::NamedTempFile::new().expect("temp do jar");
    let jar = jar_file.path().to_path_buf();
    let mut paths = base_paths(vec![(
        "FAKE_BACKEND_ENV_DUMP".into(),
        dump_path.display().to_string(),
    )]);
    paths.server_jar = Some(jar.clone());

    let manager = StreamManager::new(paths);
    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start");

    // O fake escreve assim que sobe; espera curta para o arquivo existir.
    for _ in 0..50 {
        if std::fs::read_to_string(&dump_path).is_ok_and(|s| !s.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let got = std::fs::read_to_string(&dump_path).unwrap_or_default();
    let _ = manager.stop(session_id).await;

    assert!(
        got.lines()
            .any(|l| l == format!("SCRCPY_SERVER_PATH={}", jar.display())),
        "o cliente scrcpy precisa receber SCRCPY_SERVER_PATH apontando para o jar \
         do fork; sem isso ele envia o próprio server e os controles de câmera \
         quebram com `conexão de controle encerrada pelo servidor`. Recebido: {got}"
    );
}

/// T095 (bancada 2026-10-04): o cliente scrcpy procura o `adb` DELE ao lado
/// do próprio binário antes do PATH. Desde que o scrcpy passou a ser
/// embutido (T092) ele mora em `<recursos>/bin/`, onde o adb não está — no
/// `.deb` e no Arch o adb vem da distro, e em dev o `tauri-build` copia os
/// `bundle.resources` para `target/debug/bin/`, que só tem scrcpy e o jar.
/// Dava `Could not start adb server` seis vezes seguidas, terminando em
/// "desconecte e reconecte o cabo USB". Só o AppImage escapava, porque leva
/// os três binários no mesmo diretório.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn scrcpy_client_receives_the_resolved_adb() {
    let dump = tempfile::NamedTempFile::new().expect("temp");
    let dump_path = dump.path().to_path_buf();
    let jar_file = tempfile::NamedTempFile::new().expect("temp do jar");

    let mut paths = base_paths(vec![(
        "FAKE_BACKEND_ENV_DUMP".into(),
        dump_path.display().to_string(),
    )]);
    paths.server_jar = Some(jar_file.path().to_path_buf());
    let expected_adb = paths.adb.clone();

    let manager = StreamManager::new(paths);
    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start");

    for _ in 0..50 {
        if std::fs::read_to_string(&dump_path).is_ok_and(|s| !s.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let got = std::fs::read_to_string(&dump_path).unwrap_or_default();
    let _ = manager.stop(session_id).await;

    assert!(
        got.lines()
            .any(|l| l == format!("ADB={}", expected_adb.display())),
        "o cliente scrcpy precisa receber ADB apontando para o adb que o CamLink \
         resolveu; sem isso ele tenta o adb ao lado do próprio binário, que não \
         existe no .deb nem em dev. Recebido: {got}"
    );
}

// ---------------------------------------------------------------------------
// T097 — a versão declarada ao servidor TEM que ser a do jar
//
// No Windows o CamLink é o cliente: ele passa `SCRCPY_VERSION` como
// `args[0]` do `com.genymobile.scrcpy.Server`, e o `Options.parse` do
// servidor lança `IllegalArgumentException` se não bater com o
// `BuildConfig.VERSION_NAME` dele. O processo morre antes de abrir socket.
//
// Depois do rebase do fork para a v4.1 (T092) a constante ficou em "4.0" e
// quebrou só o Windows — no Linux ela não é usada, porque lá quem declara a
// versão é o binário do scrcpy. Foi a terceira vez nesta feature que versão
// desencontrada passou por todos os gates, então a checagem deixou de ser
// humana: este teste lê o `SCRCPY_PINNED` do vendor.sh, que é a fonte única
// da versão (o mesmo valor que o guard de empacotamento compara com o
// `versionName` do fork e com o dex do jar).
// ---------------------------------------------------------------------------

#[test]
fn scrcpy_version_matches_the_pinned_client() {
    // O vendor.sh vive no repositório principal (não no submodule), então
    // está presente mesmo no checkout sem submodules que o CI usa.
    let vendor_sh =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../installer/linux/vendor.sh");
    let script = std::fs::read_to_string(&vendor_sh)
        .unwrap_or_else(|e| panic!("não consegui ler {}: {e}", vendor_sh.display()));

    let pinned = script
        .lines()
        .find_map(|l| l.strip_prefix("SCRCPY_PINNED="))
        .map(|v| v.trim().trim_matches('"').trim_start_matches('v'))
        .expect("SCRCPY_PINNED não encontrado no vendor.sh");

    assert_eq!(
        SCRCPY_VERSION, pinned,
        "SCRCPY_VERSION ({SCRCPY_VERSION}) difere do SCRCPY_PINNED do vendor.sh ({pinned}). \
         No Windows isso mata o servidor no bootstrap com `The server version (X) does not \
         match the client (Y)`: sem vídeo e sem controles. Ao rebasear o fork, atualize os \
         dois — e o `versionName` do scrcpy/server/build.gradle."
    );
}

/// T100 (bancada 2026-10-08): o `fps` das stats significa coisas diferentes
/// por plataforma, e a interface mostrava o número do Linux como se fosse o
/// do stream — anunciava ~4 fps para um stream que o log do servidor provou
/// estar a 29,8 (`frame 16016` em 537 s). No Linux os quadros vão do scrcpy
/// direto ao v4l2loopback e o contador do app é o do preview, limitado a
/// 5/s; no Windows eles passam pelo app e o número é do stream.
#[tokio::test]
async fn stats_dizem_se_o_fps_e_do_preview() {
    let jar = tempfile::NamedTempFile::new().expect("temp do jar");
    let mut paths = base_paths(Vec::new());
    paths.server_jar = Some(jar.path().to_path_buf());

    let manager = StreamManager::new(paths);
    let session_id = manager
        .start(
            SessionSource::Android("R58M12ABCDE".into()),
            sample_config(),
            "/dev/video0",
            None,
        )
        .await
        .expect("start");

    let session = manager.session(session_id).await.expect("sessão viva");
    let _ = manager.stop(session_id).await;

    assert_eq!(
        session.stats.fps_is_preview,
        cfg!(target_os = "linux"),
        "a flag tem que dizer a verdade sobre o que `fps` mede nesta \
         plataforma: no Linux é a taxa do preview, no Windows a do stream"
    );
}
