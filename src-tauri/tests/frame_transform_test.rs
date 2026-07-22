//! T075 — Testes do transform RGBA de orientação (US2 / FR-016a).
//!
//! `frame_transform::apply` é pura (sem I/O): espelha horizontalmente e/ou
//! gira em passos de 90°. Mirror e 180° preservam as dimensões (aplicáveis ao
//! vivo, sem interromper o stream); 90°/270° trocam width↔height (caminho de
//! restart, mesmo orçamento ≤ 2 s de FR-015). Ordem definida: espelha
//! PRIMEIRO (na orientação da fonte), depois gira.

use camlink_lib::frame_transform::apply;
use camlink_lib::model::{ControlState, Rotation};

/// Frame 2×2 com um pixel RGBA distinto por posição:
/// ```text
/// A B
/// C D
/// ```
const A: [u8; 4] = [1, 1, 1, 255];
const B: [u8; 4] = [2, 2, 2, 255];
const C: [u8; 4] = [3, 3, 3, 255];
const D: [u8; 4] = [4, 4, 4, 255];

fn frame_2x2() -> Vec<u8> {
    [A, B, C, D].concat()
}

fn px(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    frame[i..i + 4].try_into().unwrap()
}

#[test]
fn identity_returns_frame_unchanged() {
    let (out, w, h) = apply(&frame_2x2(), 2, 2, Rotation::Deg0, false);
    assert_eq!((w, h), (2, 2));
    assert_eq!(out, frame_2x2());
}

#[test]
fn mirror_flips_horizontally_keeping_dimensions() {
    // A B      B A
    // C D  →   D C
    let (out, w, h) = apply(&frame_2x2(), 2, 2, Rotation::Deg0, true);
    assert_eq!((w, h), (2, 2));
    assert_eq!(px(&out, w, 0, 0), B);
    assert_eq!(px(&out, w, 1, 0), A);
    assert_eq!(px(&out, w, 0, 1), D);
    assert_eq!(px(&out, w, 1, 1), C);
}

#[test]
fn rotate_180_keeps_dimensions() {
    // A B      D C
    // C D  →   B A
    let (out, w, h) = apply(&frame_2x2(), 2, 2, Rotation::Deg180, false);
    assert_eq!((w, h), (2, 2));
    assert_eq!(px(&out, w, 0, 0), D);
    assert_eq!(px(&out, w, 1, 0), C);
    assert_eq!(px(&out, w, 0, 1), B);
    assert_eq!(px(&out, w, 1, 1), A);
}

#[test]
fn mirror_plus_180_equals_vertical_flip() {
    // A B      C D
    // C D  →   A B
    let (out, w, h) = apply(&frame_2x2(), 2, 2, Rotation::Deg180, true);
    assert_eq!((w, h), (2, 2));
    assert_eq!(px(&out, w, 0, 0), C);
    assert_eq!(px(&out, w, 1, 0), D);
    assert_eq!(px(&out, w, 0, 1), A);
    assert_eq!(px(&out, w, 1, 1), B);
}

#[test]
fn rotate_90_clockwise_swaps_dimensions() {
    // 2×1: A B  →  1×2: A
    //                    B
    let frame = [A, B].concat();
    let (out, w, h) = apply(&frame, 2, 1, Rotation::Deg90, false);
    assert_eq!((w, h), (1, 2), "90° troca width↔height");
    assert_eq!(px(&out, w, 0, 0), A);
    assert_eq!(px(&out, w, 0, 1), B);
}

#[test]
fn rotate_270_swaps_dimensions() {
    // 2×1: A B  →  1×2: B
    //                    A
    let frame = [A, B].concat();
    let (out, w, h) = apply(&frame, 2, 1, Rotation::Deg270, false);
    assert_eq!((w, h), (1, 2), "270° troca width↔height");
    assert_eq!(px(&out, w, 0, 0), B);
    assert_eq!(px(&out, w, 0, 1), A);
}

#[test]
fn rotate_90_on_2x2() {
    // A B      C A
    // C D  →   D B
    let (out, w, h) = apply(&frame_2x2(), 2, 2, Rotation::Deg90, false);
    assert_eq!((w, h), (2, 2));
    assert_eq!(px(&out, w, 0, 0), C);
    assert_eq!(px(&out, w, 1, 0), A);
    assert_eq!(px(&out, w, 0, 1), D);
    assert_eq!(px(&out, w, 1, 1), B);
}

#[test]
fn rotation_helpers() {
    assert!(!Rotation::Deg0.swaps_dimensions());
    assert!(Rotation::Deg90.swaps_dimensions());
    assert!(!Rotation::Deg180.swaps_dimensions());
    assert!(Rotation::Deg270.swaps_dimensions());
}

/// T077 — `ControlState` ganha `rotation`/`mirror` com defaults neutros e
/// roundtrip serde estável (cobre o contrato do comando `set_control`).
#[test]
fn control_state_defaults_and_roundtrip() {
    let state = ControlState::default();
    assert_eq!(state.rotation, Rotation::Deg0);
    assert!(!state.mirror);

    let json = serde_json::to_string(&state).expect("serialize");
    let back: ControlState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, state);
}
