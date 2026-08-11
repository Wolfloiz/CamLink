//! T030 — Teste de contrato do protocolo de controle (US2).
//!
//! Valida os golden files de `specs/001-phone-webcam-bridge/contracts/golden/`
//! contra os tipos de fio do cliente Rust (`camera_controller`): a serialização
//! de cada `ControlRequest` deve bater byte-a-byte (como JSON) com o request
//! canônico, e cada response canônica deve parsear no tipo esperado. O fork
//! Java valida os MESMOS arquivos (ProtocolTest.java) — fonte única de verdade.

use std::fs;
use std::path::{Path, PathBuf};

use camlink_lib::camera_controller::{
    ControlErrorCode, ControlReply, ControlRequest, ServerMessage,
};
use camlink_lib::model::{DeviceCapabilities, FocusMode, SmartMode, WbMode};
use serde_json::Value;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../specs/001-phone-webcam-bridge/contracts/golden")
}

fn load(name: &str) -> Value {
    let path = golden_dir().join(name);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("golden file {} ilegível: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("golden file {name} não é JSON válido: {e}"))
}

/// Reconstrói o `ControlRequest` tipado a partir do request canônico do golden
/// file — cobre todos os comandos do contrato §2–3 (escopo US2). `None` para
/// requests que só existem como caso de validação server-side (comando/modo
/// inválido, campo ausente): o cliente tipado não consegue construí-los.
fn typed_request(req: &Value) -> Option<ControlRequest> {
    let cmd = req["cmd"].as_str().expect("request sem cmd");
    let typed = match cmd {
        "hello" => ControlRequest::Hello,
        "get_capabilities" => ControlRequest::GetCapabilities,
        "set_mode" => {
            // .ok()? em vez de .expect(...): cobre set_mode_bad_mode.json
            // (valor que não bate com nenhuma variante de SmartMode) e
            // set_mode_missing_field.json (campo ausente) — mesmo padrão de
            // set_focus_bad_mode.json acima.
            let mode: SmartMode = serde_json::from_value(req["mode"].clone()).ok()?;
            ControlRequest::SetMode { mode }
        }
        "set_zoom" => ControlRequest::SetZoom {
            ratio: req.get("ratio")?.as_f64()? as f32,
        },
        "set_focus" => {
            let focus = match req["mode"].as_str()? {
                "continuous" => FocusMode::ContinuousAuto,
                "tap" => FocusMode::Tap {
                    x: req["x"].as_f64().expect("x") as f32,
                    y: req["y"].as_f64().expect("y") as f32,
                },
                "manual" => FocusMode::Manual {
                    distance: req["distance"].as_f64().expect("distance") as f32,
                },
                _ => return None, // ex.: set_focus_bad_mode.json
            };
            ControlRequest::SetFocus { focus }
        }
        "set_exposure" => ControlRequest::SetExposure {
            compensation: req["compensation"].as_i64().expect("compensation") as i32,
        },
        "set_iso" => ControlRequest::SetIso {
            value: req["value"].as_u64().expect("value") as u32,
        },
        "set_wb" => {
            let mode: WbMode = serde_json::from_value(req["mode"].clone()).expect("wb mode");
            ControlRequest::SetWb { mode }
        }
        "set_eis" => ControlRequest::SetEis {
            enabled: req["enabled"].as_bool().expect("enabled"),
        },
        "set_torch" => ControlRequest::SetTorch {
            enabled: req["enabled"].as_bool().expect("enabled"),
        },
        "raw_snapshot" => ControlRequest::RawSnapshot,
        "raw_sequence_start" => ControlRequest::RawSequenceStart {
            // .ok()? / ? cobrem raw_sequence_start_missing_fps.json (campo
            // ausente) — mesmo padrão de set_zoom acima.
            fps: req.get("fps")?.as_f64()? as f32,
        },
        "raw_sequence_stop" => ControlRequest::RawSequenceStop,
        _ => return None, // ex.: unknown_cmd.json
    };
    Some(typed)
}

/// Todo golden file com `request` (JSON válido) e os comandos conhecidos:
/// a serialização do cliente deve produzir exatamente o request canônico.
#[test]
fn requests_match_golden_wire_format() {
    let mut checked = 0;
    for entry in fs::read_dir(golden_dir()).expect("golden dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "json") || path.is_dir() {
            continue;
        }
        let case = load(path.file_name().unwrap().to_str().unwrap());
        let Some(req) = case.get("request") else {
            continue; // ex.: invalid_json.json usa request_raw
        };
        let Some(typed) = typed_request(req) else {
            continue; // caso de validação server-side, sem tipo no cliente
        };
        let line = typed.to_line();
        assert!(
            !line.contains('\n'),
            "{}: request NDJSON não pode conter quebra de linha",
            path.display()
        );
        let reserialized: Value = serde_json::from_str(&line).expect("request serializado");
        assert_eq!(
            &reserialized,
            req,
            "{}: serialização difere do golden",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 15,
        "esperava ≥ 15 requests cobertos, veio {checked}"
    );
}

/// Toda response canônica parseia no tipo certo (envelope ok/error, hello).
#[test]
fn responses_parse_into_typed_replies() {
    for entry in fs::read_dir(golden_dir()).expect("golden dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "json") || path.is_dir() {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let case = load(&name);
        let resp = &case["response"];
        let line = serde_json::to_string(resp).expect("response serializável");
        let msg = camlink_lib::camera_controller::parse_server_line(&line)
            .unwrap_or_else(|e| panic!("{name}: response não parseou: {e}"));
        let ServerMessage::Reply(reply) = msg else {
            panic!("{name}: response parseou como evento");
        };
        match (resp["ok"].as_bool(), &reply) {
            (Some(true), ControlReply::Hello { protocol, server }) => {
                assert_eq!(*protocol, 1, "{name}");
                assert!(server.starts_with("camlink-"), "{name}");
            }
            (Some(true), ControlReply::Ok(data)) => {
                assert_eq!(data, &resp["data"], "{name}: data difere");
            }
            (Some(false), ControlReply::Err { code, msg }) => {
                let expected_code = resp["error"]["code"].as_str().unwrap();
                assert_eq!(code.as_str(), expected_code, "{name}");
                assert_eq!(msg, resp["error"]["msg"].as_str().unwrap(), "{name}");
            }
            (ok, reply) => panic!("{name}: par inesperado ok={ok:?} reply={reply:?}"),
        }
    }
}

/// Os 3 códigos de erro exigidos pelo T030 aparecem nos golden files e
/// parseiam no enum do cliente.
#[test]
fn error_codes_covered() {
    let mut seen = std::collections::BTreeSet::new();
    for entry in fs::read_dir(golden_dir()).expect("golden dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "json") || path.is_dir() {
            continue;
        }
        let case = load(path.file_name().unwrap().to_str().unwrap());
        if let Some(code) = case["response"]["error"]["code"].as_str() {
            let parsed: ControlErrorCode = code.parse().expect("código de erro conhecido");
            seen.insert(parsed.as_str().to_string());
        }
    }
    for required in ["OUT_OF_RANGE", "UNSUPPORTED", "BAD_REQUEST"] {
        assert!(seen.contains(required), "golden files sem caso {required}");
    }
}

/// `get_capabilities`: o `data` canônico deserializa em `DeviceCapabilities`
/// e o roundtrip bate com a fixture (`fixtures/capabilities_full.json`).
#[test]
fn capabilities_roundtrip_matches_fixture() {
    let case = load("get_capabilities_full.json");
    let data = case["response"]["data"].clone();
    let caps: DeviceCapabilities =
        serde_json::from_value(data.clone()).expect("data → DeviceCapabilities");
    let roundtrip = serde_json::to_value(&caps).expect("DeviceCapabilities → JSON");
    assert_eq!(roundtrip, data, "roundtrip mudou o JSON de capabilities");

    let fixture = load("fixtures/capabilities_full.json");
    assert_eq!(data, fixture, "data do golden difere da fixture");

    let minimal = load("fixtures/capabilities_minimal.json");
    let caps_min: DeviceCapabilities =
        serde_json::from_value(minimal).expect("fixture minimal → DeviceCapabilities");
    assert!(caps_min.iso_range.is_none());
    assert!(caps_min.raw.is_none());
    assert!(!caps_min.supports_torch);
}
