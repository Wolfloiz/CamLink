//! T032 — Testes do cliente de controle (US2) com servidor fake TCP.
//!
//! Cobre: handshake `hello` (versão de protocolo desconhecida → erro
//! descritivo), request/response com timeout, demux de eventos assíncronos
//! (`af_state`/`faces`) intercalados com respostas, e a montagem dos
//! argumentos do túnel `adb forward`.

use std::time::Duration;

use camlink_lib::camera_controller::{
    adb_forward_args, ControlClient, ControlClientError, ControlEvent, ControlReply, ControlRequest,
};
use camlink_lib::raw_manager::{encode_frame, RawFrameMetadata};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Servidor fake: responde cada linha recebida com as linhas de `script` na
/// ordem (uma entrada do script por request; entradas `evt:` são emitidas
/// ANTES da resposta daquele request, simulando eventos intercalados).
async fn spawn_fake_server(hello: &'static str, script: Vec<Vec<&'static str>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut script = script.into_iter();

        // Handshake: primeira linha deve ser hello.
        let first = lines.next_line().await.expect("read").expect("eof");
        assert!(
            first.contains("hello"),
            "primeira linha não é hello: {first}"
        );
        write
            .write_all(format!("{hello}\n").as_bytes())
            .await
            .expect("write hello");

        while let Ok(Some(_line)) = lines.next_line().await {
            // Script esgotado: mantém a conexão aberta sem responder (é o
            // cenário do teste de timeout — o servidor fica mudo, não cai).
            let Some(replies) = script.next() else {
                continue;
            };
            for reply in replies {
                write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .expect("write");
            }
        }
    });
    port
}

const HELLO_OK: &str = r#"{"ok":true,"protocol":1,"server":"camlink-v4.0"}"#;

#[tokio::test]
async fn handshake_and_zoom_roundtrip() {
    let port = spawn_fake_server(HELLO_OK, vec![vec![r#"{"ok":true,"data":{"ratio":2.5}}"#]]).await;

    let (mut client, _events) = ControlClient::connect(("127.0.0.1", port), Duration::from_secs(2))
        .await
        .expect("connect");

    let reply = client
        .request(ControlRequest::SetZoom { ratio: 2.5 })
        .await
        .expect("set_zoom");
    match reply {
        ControlReply::Ok(data) => assert_eq!(data["ratio"], 2.5),
        other => panic!("esperava Ok, veio {other:?}"),
    }
}

#[tokio::test]
async fn unknown_protocol_version_is_rejected_with_descriptive_error() {
    let port = spawn_fake_server(
        r#"{"ok":true,"protocol":99,"server":"camlink-v9.9"}"#,
        vec![],
    )
    .await;

    let err = ControlClient::connect(("127.0.0.1", port), Duration::from_secs(2))
        .await
        .expect_err("protocol 99 deveria ser rejeitado");
    match err {
        ControlClientError::UnsupportedProtocol { got } => assert_eq!(got, 99),
        other => panic!("esperava UnsupportedProtocol, veio {other:?}"),
    }
    // Mensagem acionável: menciona a versão do fork (FR-010 análogo).
    let msg = format!("{}", ControlClientError::UnsupportedProtocol { got: 99 });
    assert!(msg.contains("99"), "mensagem sem a versão recebida: {msg}");
}

#[tokio::test]
async fn events_are_demuxed_from_replies() {
    // O servidor emite af_state e faces ANTES da resposta do request: o
    // cliente deve entregar a resposta ao chamador e os eventos no canal.
    let port = spawn_fake_server(
        HELLO_OK,
        vec![vec![
            r#"{"event":"af_state","state":"searching"}"#,
            r#"{"event":"faces","rects":[{"x":0.4,"y":0.2,"w":0.1,"h":0.15}]}"#,
            r#"{"ok":true,"data":{"mode":"tap","x":0.5,"y":0.3}}"#,
        ]],
    )
    .await;

    let (mut client, mut events) =
        ControlClient::connect(("127.0.0.1", port), Duration::from_secs(2))
            .await
            .expect("connect");

    let reply = client
        .request(ControlRequest::SetFocus {
            focus: camlink_lib::model::FocusMode::Tap { x: 0.5, y: 0.3 },
        })
        .await
        .expect("set_focus");
    assert!(matches!(reply, ControlReply::Ok(_)));

    let first = events.recv().await.expect("evento af_state");
    match first {
        ControlEvent::AfState { state } => assert_eq!(state, "searching"),
        other => panic!("esperava AfState, veio {other:?}"),
    }
    let second = events.recv().await.expect("evento faces");
    match second {
        ControlEvent::Faces { rects } => {
            assert_eq!(rects.len(), 1);
            assert!((rects[0].x - 0.4).abs() < f32::EPSILON);
        }
        other => panic!("esperava Faces, veio {other:?}"),
    }
}

#[tokio::test]
async fn request_times_out_when_server_never_replies() {
    // Script vazio: o servidor lê o request mas nunca responde.
    let port = spawn_fake_server(HELLO_OK, vec![]).await;

    let (mut client, _events) =
        ControlClient::connect(("127.0.0.1", port), Duration::from_millis(200))
            .await
            .expect("connect");

    let err = client
        .request(ControlRequest::SetTorch { enabled: true })
        .await
        .expect_err("deveria estourar timeout");
    assert!(
        matches!(err, ControlClientError::Timeout),
        "esperava Timeout, veio {err:?}"
    );
}

#[tokio::test]
async fn raw_frame_binary_framing_is_demuxed_between_json_lines() {
    // Contrato §4: o frame RAW binário (tag 0xD1) chega no MESMO socket que
    // as linhas NDJSON — precisa continuar lendo eventos/respostas de texto
    // normalmente antes e depois do frame binário no meio.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let metadata = RawFrameMetadata {
        seq: 3,
        timestamp_ms: 1_785_800_000_000,
        width: 4032,
        height: 3024,
    };
    let dng = vec![0xABu8; 256];
    let framed = encode_frame(&metadata, &dng);

    tokio::spawn({
        let framed = framed.clone();
        async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();

            let first = lines.next_line().await.expect("read").expect("eof");
            assert!(first.contains("hello"));
            write
                .write_all(format!("{HELLO_OK}\n").as_bytes())
                .await
                .expect("write hello");

            // request do teste (raw_sequence_start)
            let _ = lines.next_line().await.expect("read").expect("eof");

            // Evento de texto ANTES do frame binário.
            write
                .write_all(b"{\"event\":\"af_state\",\"state\":\"searching\"}\n")
                .await
                .expect("write af_state");
            // O frame binário em si (sem terminador de linha — é length-prefixed).
            write.write_all(&framed).await.expect("write raw frame");
            // Resposta do request, DEPOIS do frame binário no mesmo socket.
            write
                .write_all(b"{\"ok\":true,\"data\":{\"granted_fps\":2.0}}\n")
                .await
                .expect("write reply");
        }
    });

    let (mut client, mut events) =
        ControlClient::connect(("127.0.0.1", port), Duration::from_secs(2))
            .await
            .expect("connect");

    let reply = client
        .request(ControlRequest::RawSequenceStart { fps: 3.0 })
        .await
        .expect("raw_sequence_start");
    match reply {
        ControlReply::Ok(data) => assert_eq!(data["granted_fps"], 2.0),
        other => panic!("esperava Ok, veio {other:?}"),
    }

    let first_event = events.recv().await.expect("evento af_state");
    assert!(matches!(first_event, ControlEvent::AfState { .. }));

    let second_event = events.recv().await.expect("evento RawFrame");
    match second_event {
        ControlEvent::RawFrame(frame) => {
            assert_eq!(frame.metadata, metadata);
            assert_eq!(frame.dng, dng);
        }
        other => panic!("esperava RawFrame, veio {other:?}"),
    }
}

#[test]
fn adb_forward_args_shape() {
    let args = adb_forward_args("SERIAL123", 27184);
    assert_eq!(
        args,
        vec![
            "-s".to_string(),
            "SERIAL123".to_string(),
            "forward".to_string(),
            "tcp:27184".to_string(),
            "localabstract:camlink".to_string(),
        ]
    );
}
