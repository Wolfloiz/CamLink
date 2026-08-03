//! T043 — Testa que `ControlState::apply_smart_mode` reflete a tabela de
//! modos (research.md R2 / control-protocol.md §3) nos únicos 2 campos que
//! têm representação dedicada em `ControlState`: `eis` e `exposure_comp`
//! (o resto da tabela — AF/AE/FPS/AWB/NR — não tem campo próprio na UI,
//! data-model.md). `mode` em si é sempre sobrescrito.

use camlink_lib::model::{ControlState, SmartMode};

#[test]
fn auto_enables_eis_and_zeroes_exposure() {
    let mut state = ControlState::default();
    state.apply_smart_mode(SmartMode::Auto);
    assert!(state.eis);
    assert_eq!(state.exposure_comp, 0);
    assert_eq!(state.mode, SmartMode::Auto);
}

#[test]
fn night_enables_eis_and_sets_plus_one_ev() {
    let mut state = ControlState::default();
    state.apply_smart_mode(SmartMode::Night);
    assert!(state.eis);
    assert_eq!(state.exposure_comp, 1);
    assert_eq!(state.mode, SmartMode::Night);
}

#[test]
fn sport_disables_eis_and_zeroes_exposure() {
    let mut state = ControlState {
        exposure_comp: 5, // valor comandado antes de entrar em sport
        ..Default::default()
    };
    state.apply_smart_mode(SmartMode::Sport);
    assert!(!state.eis);
    assert_eq!(state.exposure_comp, 0);
    assert_eq!(state.mode, SmartMode::Sport);
}

#[test]
fn pro_disables_eis_but_leaves_exposure_comp_free() {
    // AE_EXPOSURE_COMPENSATION é "livre" pra pro na tabela (só fps/EV/AWB são
    // livres) — mas VIDEO_STABILIZATION_MODE_OFF é fixo, não livre.
    let mut state = ControlState {
        exposure_comp: -2, // valor comandado manualmente antes de pro
        ..Default::default()
    };
    state.apply_smart_mode(SmartMode::Pro);
    assert!(!state.eis);
    assert_eq!(
        state.exposure_comp, -2,
        "pro é livre pra exposure_comp: apply_smart_mode não deve tocar nele"
    );
    assert_eq!(state.mode, SmartMode::Pro);
}

#[test]
fn switching_modes_always_overwrites_mode_field() {
    let mut state = ControlState::default();
    for mode in [
        SmartMode::Auto,
        SmartMode::Night,
        SmartMode::Sport,
        SmartMode::Pro,
    ] {
        state.apply_smart_mode(mode);
        assert_eq!(state.mode, mode);
    }
}
