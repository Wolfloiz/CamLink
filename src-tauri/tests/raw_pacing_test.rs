//! T056 — Teste do cálculo de cadência dinâmica da Sequência RAW (US5,
//! FR-019/020): fps sustentável = banda disponível ÷ tamanho do frame DNG,
//! sempre entre 1 e 3 fps, com o stream de vídeo principal tendo prioridade
//! sobre a banda disputada.

use camlink_lib::raw_manager::{
    effective_raw_fps, granted_fps, throughput_for_raw, RAW_SEQUENCE_MAX_FPS, RAW_SEQUENCE_MIN_FPS,
};

// ---------------------------------------------------------------------------
// Banda reservada pro stream principal (FR-020)
// ---------------------------------------------------------------------------

#[test]
fn raw_gets_only_the_leftover_after_main_stream() {
    let total = 10_000_000.0; // 10 MB/s medidos no túnel ADB
    let main_stream = 6_000_000.0; // stream principal consumindo 6 MB/s
    assert_eq!(throughput_for_raw(total, main_stream), 4_000_000.0);
}

#[test]
fn main_stream_spike_never_yields_negative_bandwidth_for_raw() {
    // Pico transitório: stream principal momentaneamente "usa" mais do que o
    // total medido (medição atrasada) — RAW não pode ficar com banda negativa.
    let total = 5_000_000.0;
    let main_stream = 8_000_000.0;
    assert_eq!(throughput_for_raw(total, main_stream), 0.0);
}

// ---------------------------------------------------------------------------
// fps sustentável = throughput ÷ frame_bytes, clampado a [1,3]
// ---------------------------------------------------------------------------

#[test]
fn computes_sustainable_fps_from_frame_size_and_throughput() {
    // Frame de 2 MB, 4 MB/s disponíveis pra RAW → 2 fps sustentável.
    let fps = effective_raw_fps(2_000_000, 4_000_000.0);
    assert!((fps - 2.0).abs() < 0.01, "esperava ~2.0, veio {fps}");
}

#[test]
fn clamps_to_max_when_bandwidth_easily_exceeds_frame_size() {
    // Banda de sobra pro tamanho do frame — nunca ultrapassa o teto de 3 fps.
    let fps = effective_raw_fps(1_000_000, 100_000_000.0);
    assert_eq!(fps, RAW_SEQUENCE_MAX_FPS);
}

#[test]
fn clamps_to_min_when_bandwidth_is_scarce() {
    // Banda muito menor do que 1 frame/s — nunca cai abaixo do piso de 1 fps
    // (best-effort; quem decide desistir do job é uma camada acima).
    let fps = effective_raw_fps(5_000_000, 100_000.0);
    assert_eq!(fps, RAW_SEQUENCE_MIN_FPS);
}

#[test]
fn zero_bandwidth_still_floors_at_minimum_fps() {
    let fps = effective_raw_fps(2_000_000, 0.0);
    assert_eq!(fps, RAW_SEQUENCE_MIN_FPS);
}

#[test]
fn zero_frame_size_never_divides_by_zero() {
    // Não deveria acontecer na prática (sensor sempre produz algo), mas a
    // função não pode panicar/retornar NaN/infinito por isso.
    let fps = effective_raw_fps(0, 4_000_000.0);
    assert!(fps.is_finite());
    assert_eq!(fps, RAW_SEQUENCE_MAX_FPS);
}

#[test]
fn fps_is_always_within_the_one_to_three_range() {
    let cases: &[(u64, f64)] = &[
        (100, 1.0),
        (10_000_000, 50_000_000.0),
        (500_000, 500_000.0),
        (3_000_000, 0.0),
    ];
    for &(frame_bytes, throughput) in cases {
        let fps = effective_raw_fps(frame_bytes, throughput);
        assert!(
            (RAW_SEQUENCE_MIN_FPS..=RAW_SEQUENCE_MAX_FPS).contains(&fps),
            "fps {fps} fora de [1,3] para frame_bytes={frame_bytes}, throughput={throughput}"
        );
    }
}

// ---------------------------------------------------------------------------
// `granted_fps` — resposta ao `raw_sequence_start` (contracts §4)
// ---------------------------------------------------------------------------

#[test]
fn granted_fps_matches_request_when_bandwidth_allows() {
    // Pediu 2 fps, banda sobra pra 3 — concede exatamente o pedido, não mais.
    let fps = granted_fps(2.0, 1_000_000, 10_000_000.0);
    assert!((fps - 2.0).abs() < 0.01, "esperava ~2.0, veio {fps}");
}

#[test]
fn granted_fps_is_reduced_below_request_when_bandwidth_is_insufficient() {
    // Pediu 3 fps (contracts/control-protocol.md exemplo), mas a banda só
    // sustenta ~1 fps — servidor deve responder com `granted_fps` reduzido,
    // nunca com o valor pedido.
    let fps = granted_fps(3.0, 5_000_000, 4_000_000.0);
    assert!(fps < 3.0, "deveria reduzir abaixo do pedido, veio {fps}");
    assert!(fps >= RAW_SEQUENCE_MIN_FPS);
}

#[test]
fn granted_fps_never_exceeds_what_was_requested_even_with_excess_bandwidth() {
    // Pediu 1 fps só; mesmo com banda de sobra pra 3, não faz sentido gravar
    // mais rápido do que o cliente pediu.
    let fps = granted_fps(1.0, 100_000, 100_000_000.0);
    assert_eq!(fps, RAW_SEQUENCE_MIN_FPS);
}

#[test]
fn requested_fps_outside_valid_range_is_clamped_before_granting() {
    let too_high = granted_fps(10.0, 100, 100_000_000.0);
    assert_eq!(too_high, RAW_SEQUENCE_MAX_FPS);

    let too_low = granted_fps(0.1, 100, 100_000_000.0);
    assert_eq!(too_low, RAW_SEQUENCE_MIN_FPS);
}
