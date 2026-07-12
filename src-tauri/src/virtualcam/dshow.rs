//! Backend Windows: filtro DirectShow próprio + push de frames.
//!
//! Substitui o backend FFI via akvirtualcamera (reprovado no Spike B —
//! ver `specs/001-phone-webcam-bridge/research.md` R4). Implementação
//! consolidada em T023, gateada pela Spike C (T074,
//! `src-tauri/examples/win_dshow_spike.rs`).
