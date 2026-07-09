//! T010 — Testes do trait `VirtualCameraBackend` com backend mock
//! (create/feed_frame/set_standby/destroy; invariante 1 fonte ↔ 1 device).
//!
//! Escritos ANTES da implementação (Princípio III); falham até T014
//! implementar `src-tauri/src/virtualcam/mod.rs` (trait + mock + gerador de
//! imagem de espera — FR-006).

use camlink_lib::model::VirtualCameraState;
use camlink_lib::virtualcam::{standby_frame, MockBackend, VirtualCameraBackend};

const RES: (u32, u32) = (1280, 720);
const FPS: u32 = 30;

/// Frame RGBA sintético do tamanho exato da resolução.
fn frame_for(resolution: (u32, u32)) -> Vec<u8> {
    vec![0x7F; (resolution.0 * resolution.1 * 4) as usize]
}

#[test]
fn create_returns_camera_in_standby() {
    let mut backend = MockBackend::new();
    let cam = backend
        .create("CamLink Android", RES, FPS)
        .expect("create falhou");

    assert_eq!(cam.label, "CamLink Android");
    assert!(!cam.backend_path.is_empty());
    // FR-006: device nasce em Standby (imagem de espera) até o primeiro frame.
    assert_eq!(cam.state, VirtualCameraState::Standby);

    let found = backend
        .camera(&cam.id)
        .expect("camera() não encontrou o device");
    assert_eq!(found.id, cam.id);
}

#[test]
fn feed_frame_switches_to_live() {
    let mut backend = MockBackend::new();
    let cam = backend.create("CamLink Android", RES, FPS).expect("create");

    backend
        .feed_frame(&cam.id, &frame_for(RES))
        .expect("feed_frame falhou");

    let cam = backend.camera(&cam.id).expect("camera()");
    assert_eq!(cam.state, VirtualCameraState::Live);
    assert_eq!(backend.frames_fed(&cam.id), 1);
}

#[test]
fn set_standby_switches_back_and_records_message() {
    let mut backend = MockBackend::new();
    let cam = backend.create("CamLink Android", RES, FPS).expect("create");
    backend.feed_frame(&cam.id, &frame_for(RES)).expect("feed");

    backend
        .set_standby(&cam.id, "Aguardando dispositivo…")
        .expect("set_standby falhou");

    let found = backend.camera(&cam.id).expect("camera()");
    assert_eq!(found.state, VirtualCameraState::Standby);
    // FR-006: a imagem de espera carrega a mensagem de estado.
    assert_eq!(
        backend.last_standby_message(&cam.id).as_deref(),
        Some("Aguardando dispositivo…")
    );
}

#[test]
fn destroy_removes_device_and_rejects_further_ops() {
    let mut backend = MockBackend::new();
    let cam = backend.create("CamLink Android", RES, FPS).expect("create");

    backend.destroy(&cam.id).expect("destroy falhou");
    assert!(backend.camera(&cam.id).is_none());

    // Operações sobre device destruído são erro, nunca panic.
    assert!(backend.feed_frame(&cam.id, &frame_for(RES)).is_err());
    assert!(backend.set_standby(&cam.id, "x").is_err());
    assert!(backend.destroy(&cam.id).is_err());
}

#[test]
fn empty_frame_is_rejected() {
    let mut backend = MockBackend::new();
    let cam = backend.create("CamLink Android", RES, FPS).expect("create");
    assert!(backend.feed_frame(&cam.id, &[]).is_err());
}

#[test]
fn one_source_one_device_invariant() {
    // Invariante (data-model.md / FR-021): 1 fonte ativa ↔ 1 device virtual.
    // Cada create produz um device distinto e independente.
    let mut backend = MockBackend::new();
    let a = backend
        .create("CamLink Android", RES, FPS)
        .expect("create a");
    let b = backend.create("CamLink RTSP", RES, FPS).expect("create b");

    assert_ne!(a.id, b.id);
    assert_ne!(a.backend_path, b.backend_path);
    assert_eq!(backend.cameras().len(), 2);

    // Falha/remoção de um device não afeta o outro (isolamento — FR-021).
    backend.feed_frame(&b.id, &frame_for(RES)).expect("feed b");
    backend.destroy(&a.id).expect("destroy a");
    assert!(backend.camera(&a.id).is_none());
    let b_after = backend.camera(&b.id).expect("b sobrevive ao destroy de a");
    assert_eq!(b_after.state, VirtualCameraState::Live);
    assert_eq!(backend.cameras().len(), 1);
}

#[test]
fn backend_is_object_safe() {
    // O stream/rtsp manager seleciona o backend por plataforma em runtime;
    // o trait precisa ser utilizável como objeto.
    let mut backend: Box<dyn VirtualCameraBackend> = Box::new(MockBackend::new());
    let cam = backend
        .create("CamLink Android", RES, FPS)
        .expect("create via dyn");
    backend
        .feed_frame(&cam.id, &frame_for(RES))
        .expect("feed via dyn");
    backend.destroy(&cam.id).expect("destroy via dyn");
}

#[test]
fn standby_frame_matches_resolution() {
    // Gerador da imagem de espera (FR-006): buffer RGBA do tamanho exato.
    for resolution in [(640, 480), (1280, 720), (1920, 1080)] {
        let frame = standby_frame(resolution, "Aguardando dispositivo…");
        assert_eq!(
            frame.len(),
            (resolution.0 * resolution.1 * 4) as usize,
            "frame de espera com tamanho errado para {resolution:?}"
        );
    }
}

#[test]
fn standby_frame_is_deterministic() {
    let a = standby_frame((640, 480), "Aguardando dispositivo…");
    let b = standby_frame((640, 480), "Aguardando dispositivo…");
    assert_eq!(a, b, "imagem de espera deve ser estática/determinística");
}
