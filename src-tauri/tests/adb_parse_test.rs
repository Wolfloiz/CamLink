//! T017 — Testes do parser de `adb devices -l` e do gate de compatibilidade
//! Android 12+ (FR-002a). Escritos ANTES da implementação (Princípio III);
//! falham até T021 implementar `src-tauri/src/device_manager.rs`.

use camlink_lib::device_manager::{classify_sdk, diff_devices, parse_devices_l, DeviceEvent};
use camlink_lib::model::AuthState;

#[test]
fn parses_authorized_device_with_model() {
    let output = "List of devices attached\n\
R58M12ABCDE            device usb:1-1 product:r8qxxx model:SM_G781B device:r8q transport_id:5\n";

    let devices = parse_devices_l(output);
    assert_eq!(devices.len(), 1);
    let d = &devices[0];
    assert_eq!(d.serial, "R58M12ABCDE");
    assert_eq!(d.model, "SM_G781B");
    assert_eq!(d.auth_state, AuthState::Authorized);
}

#[test]
fn parses_unauthorized_device() {
    let output = "List of devices attached\n\
1234567890AB            unauthorized usb:1-2 transport_id:6\n";

    let devices = parse_devices_l(output);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].serial, "1234567890AB");
    assert_eq!(devices[0].auth_state, AuthState::Unauthorized);
}

#[test]
fn parses_offline_device_without_extra_fields() {
    let output = "List of devices attached\n\
ABCDEF123456            offline\n";

    let devices = parse_devices_l(output);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].serial, "ABCDEF123456");
    assert_eq!(devices[0].auth_state, AuthState::Offline);
}

#[test]
fn parses_multiple_devices_mixed_states() {
    let output = "List of devices attached\n\
R58M12ABCDE            device usb:1-1 product:r8qxxx model:SM_G781B device:r8q transport_id:5\n\
1234567890AB            unauthorized usb:1-2 transport_id:6\n\
ABCDEF123456            offline\n";

    let devices = parse_devices_l(output);
    assert_eq!(devices.len(), 3);
    assert_eq!(devices[0].auth_state, AuthState::Authorized);
    assert_eq!(devices[1].auth_state, AuthState::Unauthorized);
    assert_eq!(devices[2].auth_state, AuthState::Offline);
}

#[test]
fn ignores_header_and_blank_lines() {
    let output = "List of devices attached\n\n\nR58M12ABCDE            device model:SM_G781B\n\n";
    let devices = parse_devices_l(output);
    assert_eq!(devices.len(), 1);
}

#[test]
fn empty_output_yields_no_devices() {
    let output = "List of devices attached\n";
    assert!(parse_devices_l(output).is_empty());
}

#[test]
fn device_without_model_field_falls_back_to_serial() {
    let output = "List of devices attached\n\
R58M12ABCDE            device usb:1-1 transport_id:5\n";
    let devices = parse_devices_l(output);
    assert_eq!(devices[0].model, "R58M12ABCDE");
}

// ---------------------------------------------------------------------------
// Gate Android 12+ (FR-002a) — SDK 31 é o mínimo (Android 12).
// ---------------------------------------------------------------------------

#[test]
fn sdk_31_is_compatible() {
    let (compatible, reason) = classify_sdk(31);
    assert!(compatible);
    assert!(reason.is_none());
}

#[test]
fn sdk_above_31_is_compatible() {
    let (compatible, reason) = classify_sdk(34);
    assert!(compatible);
    assert!(reason.is_none());
}

#[test]
fn sdk_below_31_is_incompatible_with_reason() {
    let (compatible, reason) = classify_sdk(30);
    assert!(!compatible);
    let reason = reason.expect("motivo obrigatório quando incompatível (FR-002a)");
    assert!(!reason.is_empty());
}

#[test]
fn sdk_very_old_is_incompatible() {
    let (compatible, _) = classify_sdk(21);
    assert!(!compatible);
}

// ---------------------------------------------------------------------------
// diff_devices — eventos de hotplug (device_connected/disconnected/unauthorized)
// ---------------------------------------------------------------------------

fn device(serial: &str, auth: AuthState) -> camlink_lib::model::AndroidDevice {
    camlink_lib::model::AndroidDevice {
        serial: serial.to_string(),
        model: "Modelo".to_string(),
        auth_state: auth,
        compatible: true,
        incompat_reason: None,
        capabilities: None,
    }
}

#[test]
fn diff_emits_connected_for_new_authorized_device() {
    let previous = vec![];
    let current = vec![device("A1", AuthState::Authorized)];
    let events = diff_devices(&previous, &current);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], DeviceEvent::Connected(d) if d.serial == "A1"));
}

#[test]
fn diff_emits_unauthorized_for_new_unauthorized_device() {
    let previous = vec![];
    let current = vec![device("A1", AuthState::Unauthorized)];
    let events = diff_devices(&previous, &current);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], DeviceEvent::Unauthorized(serial) if serial == "A1"));
}

#[test]
fn diff_emits_disconnected_for_removed_device() {
    let previous = vec![device("A1", AuthState::Authorized)];
    let current = vec![];
    let events = diff_devices(&previous, &current);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], DeviceEvent::Disconnected(serial) if serial == "A1"));
}

#[test]
fn diff_emits_nothing_when_state_unchanged() {
    let previous = vec![device("A1", AuthState::Authorized)];
    let current = vec![device("A1", AuthState::Authorized)];
    assert!(diff_devices(&previous, &current).is_empty());
}

#[test]
fn diff_emits_unauthorized_transition() {
    let previous = vec![device("A1", AuthState::Unauthorized)];
    let current = vec![device("A1", AuthState::Authorized)];
    let events = diff_devices(&previous, &current);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], DeviceEvent::Connected(d) if d.serial == "A1"));
}
