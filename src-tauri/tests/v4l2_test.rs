//! T019 — Testes do backend `virtualcam::v4l2` (Linux-only, Princípio IV):
//! parsing da saída de `v4l2loopback-ctl` (`add`/`list`/`--version`),
//! fallback para `modprobe` quando a versão é < 0.13 (sem verbo `add`
//! dinâmico confiável) e detecção do bloqueio por Secure Boot
//! (`modprobe: ... Key was rejected by service`, research.md R3).
//!
//! Formatos verificados contra o código-fonte real do
//! `v4l2loopback-ctl.c` (upstream v4l2loopback/v4l2loopback, 2026-07-13):
//! `add` sem device explícito cria um device livre e imprime só
//! `/dev/videoN\n` no stdout; `list` imprime `OUTPUT\tCAPTURE\tNAME` só no
//! stderr (não no stdout, então não há cabeçalho para pular ao parsear
//! stdout) e cada linha de dado é
//! `/dev/video%-3d\t/dev/video%-3d\t<name>\n`; `--version` imprime
//! `<progname> v<major>.<minor>.<bugfix>\n`.
//!
//! Escritos ANTES da implementação (Princípio III); falham até T022
//! implementar `src-tauri/src/virtualcam/v4l2.rs`. Este arquivo só compila
//! no Linux (o módulo `virtualcam::v4l2` é `#[cfg(target_os = "linux")]`);
//! em outras plataformas o binário de teste fica vazio.

#![cfg(target_os = "linux")]

use camlink_lib::virtualcam::v4l2::{
    build_add_args, ctl_supports_dynamic_add, detect_secure_boot_block, find_reusable_device,
    parse_added_device, parse_ctl_version, parse_list_output, LoopbackDevice,
};

// ---------------------------------------------------------------------------
// parse_added_device — saída de `v4l2loopback-ctl add -n <label> -x 1`
// ---------------------------------------------------------------------------

#[test]
fn parses_device_path_from_add_output() {
    assert_eq!(
        parse_added_device("/dev/video7\n"),
        Some("/dev/video7".to_string())
    );
}

#[test]
fn parses_device_path_without_trailing_newline() {
    assert_eq!(
        parse_added_device("/dev/video0"),
        Some("/dev/video0".to_string())
    );
}

#[test]
fn add_output_without_device_path_is_none() {
    assert_eq!(parse_added_device("failed to create device\n"), None);
}

// ---------------------------------------------------------------------------
// build_add_args — sempre cria device livre (sem número explícito, R3:
// "alocação dinâmica") com exclusive_caps=1 (mandatório p/ Chrome/OBS)
// ---------------------------------------------------------------------------

#[test]
fn add_args_request_dynamic_device_with_exclusive_caps() {
    let args = build_add_args("CamLink Android");
    assert_eq!(args, vec!["add", "-n", "CamLink Android", "-x", "1"]);
    // Nenhum <outputdevice> posicional: o kernel/ctl escolhe um device livre.
    assert!(!args.iter().any(|a| a.starts_with("/dev/video")));
}

// ---------------------------------------------------------------------------
// parse_list_output — uma linha por device, campos separados por tab
// ---------------------------------------------------------------------------

#[test]
fn parses_single_device_from_list_output() {
    let stdout = "/dev/video0  \t/dev/video0  \tCamLink Android\n";
    let devices = parse_list_output(stdout);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].output, "/dev/video0");
    assert_eq!(devices[0].capture, "/dev/video0");
    assert_eq!(devices[0].name, "CamLink Android");
}

#[test]
fn parses_multiple_devices_from_list_output() {
    let stdout = "/dev/video0  \t/dev/video0  \tCamLink Android\n\
/dev/video12 \t/dev/video12 \tOutra Fonte\n";
    let devices = parse_list_output(stdout);
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[1].output, "/dev/video12");
    assert_eq!(devices[1].name, "Outra Fonte");
}

#[test]
fn empty_list_output_yields_no_devices() {
    assert!(parse_list_output("").is_empty());
}

#[test]
fn list_output_ignores_blank_lines() {
    let stdout = "/dev/video0  \t/dev/video0  \tCamLink Android\n\n";
    assert_eq!(parse_list_output(stdout).len(), 1);
}

// ---------------------------------------------------------------------------
// find_reusable_device — device estável entre sessões: consumidores
// (OBS/Meet) mantêm a referência ao MESMO /dev/videoN entre trocas de
// câmera e restarts; criar um device por sessão fazia o Chrome listar
// duplicatas mortas ("câmera indisponível") e o OBS congelar no último
// frame do device deletado.
// ---------------------------------------------------------------------------

fn dev(path: &str, name: &str) -> LoopbackDevice {
    LoopbackDevice {
        output: path.to_string(),
        capture: path.to_string(),
        name: name.to_string(),
    }
}

#[test]
fn reuses_first_device_with_matching_label() {
    let devices = vec![
        dev("/dev/video2", "Outra Fonte"),
        dev("/dev/video5", "CamLink Android"),
        dev("/dev/video6", "CamLink Android"),
    ];
    assert_eq!(
        find_reusable_device(&devices, "CamLink Android"),
        Some("/dev/video5".to_string())
    );
}

#[test]
fn no_device_with_matching_label_yields_none() {
    let devices = vec![dev("/dev/video2", "Outra Fonte")];
    assert_eq!(find_reusable_device(&devices, "CamLink Android"), None);
    assert_eq!(find_reusable_device(&[], "CamLink Android"), None);
}

#[test]
fn label_match_is_exact_not_prefix() {
    let devices = vec![dev("/dev/video2", "CamLink Android (old)")];
    assert_eq!(find_reusable_device(&devices, "CamLink Android"), None);
}

// ---------------------------------------------------------------------------
// parse_ctl_version / ctl_supports_dynamic_add — gate de fallback (R3)
// ---------------------------------------------------------------------------

#[test]
fn parses_version_from_version_output() {
    assert_eq!(
        parse_ctl_version("v4l2loopback-ctl v0.13.1\n"),
        Some((0, 13, 1))
    );
}

#[test]
fn parses_version_with_different_program_name() {
    assert_eq!(parse_ctl_version("ctl v0.12.7\n"), Some((0, 12, 7)));
}

#[test]
fn unparseable_version_output_is_none() {
    assert_eq!(parse_ctl_version("command not found\n"), None);
}

#[test]
fn version_0_13_0_supports_dynamic_add() {
    assert!(ctl_supports_dynamic_add(Some((0, 13, 0))));
}

#[test]
fn version_above_0_13_supports_dynamic_add() {
    assert!(ctl_supports_dynamic_add(Some((0, 14, 2))));
    assert!(ctl_supports_dynamic_add(Some((1, 0, 0))));
}

#[test]
fn version_below_0_13_falls_back() {
    assert!(!ctl_supports_dynamic_add(Some((0, 12, 7))));
}

#[test]
fn missing_ctl_binary_falls_back() {
    assert!(!ctl_supports_dynamic_add(None));
}

// ---------------------------------------------------------------------------
// detect_secure_boot_block — modprobe recusando módulo não assinado
// ---------------------------------------------------------------------------

#[test]
fn detects_secure_boot_key_rejection() {
    let stderr = "modprobe: ERROR: could not insert 'v4l2loopback': Key was rejected by service\n";
    let err = detect_secure_boot_block(stderr).expect("deve detectar Secure Boot");
    assert_eq!(err.code, "secure_boot_blocked");
    assert!(
        err.action_hint.is_some(),
        "erro de Secure Boot deve ter action_hint (guia de assinatura do módulo)"
    );
}

#[test]
fn unrelated_modprobe_error_is_not_secure_boot() {
    let stderr = "modprobe: FATAL: Module v4l2loopback not found in directory /lib/modules\n";
    assert!(detect_secure_boot_block(stderr).is_none());
}

#[test]
fn empty_stderr_is_not_secure_boot() {
    assert!(detect_secure_boot_block("").is_none());
}
