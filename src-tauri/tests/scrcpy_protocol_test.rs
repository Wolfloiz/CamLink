//! Testes do parser do protocolo de socket de vídeo scrcpy usado no
//! bootstrap direto do Windows (research.md R12). Layout de bytes
//! verificado contra `scrcpy/doc/develop.md` §Protocol e
//! `scrcpy/server/.../device/DesktopConnection.java` (submodule, tag
//! v4.0) — não deduzido. Fecha a lacuna deixada em T024: a leitura do
//! socket de vídeo/decode não tinha nenhum teste até agora.
//!
//! Pura montagem/parsing de bytes, sem subprocess nem rede: roda em
//! qualquer plataforma-host (assim como T020).

use camlink_lib::stream_manager::{
    parse_device_name, parse_forward_port, parse_frame_header, parse_session_packet, CODEC_ID_H264,
    CODEC_ID_H265, DEVICE_NAME_FIELD_LENGTH,
};

// ---------------------------------------------------------------------------
// parse_device_name — campo fixo de 64 bytes, UTF-8, preenchido com zeros.
// ---------------------------------------------------------------------------

#[test]
fn parses_device_name_truncated_at_first_null() {
    let mut buf = [0u8; DEVICE_NAME_FIELD_LENGTH];
    let name = b"Galaxy S20 FE";
    buf[..name.len()].copy_from_slice(name);
    assert_eq!(parse_device_name(&buf), "Galaxy S20 FE");
}

#[test]
fn parses_device_name_filling_the_whole_field() {
    let mut buf = [b'x'; DEVICE_NAME_FIELD_LENGTH];
    // Sem terminador nulo: usa o buffer inteiro.
    assert_eq!(parse_device_name(&buf).len(), DEVICE_NAME_FIELD_LENGTH);
    buf[DEVICE_NAME_FIELD_LENGTH - 1] = 0;
    assert_eq!(parse_device_name(&buf).len(), DEVICE_NAME_FIELD_LENGTH - 1);
}

#[test]
fn parses_empty_device_name() {
    let buf = [0u8; DEVICE_NAME_FIELD_LENGTH];
    assert_eq!(parse_device_name(&buf), "");
}

// ---------------------------------------------------------------------------
// parse_session_packet — 12 bytes: bit7 do byte0 = flag de sessão; bit0 do
// byte3 = client-resized; bytes4..8 = width BE; bytes8..12 = height BE.
// ---------------------------------------------------------------------------

#[test]
fn parses_session_packet_1920x1080() {
    // byte0=0x80 (flag de sessão, sem outros bits), bytes1-3=0 (padding +
    // client_resized=0), width=1920 (0x00000780), height=1080 (0x00000438).
    let bytes: [u8; 12] = [
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x00, 0x04, 0x38,
    ];
    let session = parse_session_packet(&bytes).expect("deve reconhecer pacote de sessão");
    assert_eq!(session.width, 1920);
    assert_eq!(session.height, 1080);
    assert!(!session.client_resized);
}

#[test]
fn parses_session_packet_with_client_resized_flag() {
    let bytes: [u8; 12] = [
        0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x02, 0x80, 0x00, 0x00, 0x01, 0xE0,
    ];
    let session = parse_session_packet(&bytes).expect("deve reconhecer pacote de sessão");
    assert!(session.client_resized);
    assert_eq!(session.width, 640);
    assert_eq!(session.height, 480);
}

#[test]
fn frame_header_bytes_are_not_a_session_packet() {
    // Media packet flag (bit7 do byte0) = 0 → não é pacote de sessão.
    let bytes: [u8; 12] = [0x00; 12];
    assert!(parse_session_packet(&bytes).is_none());
}

// ---------------------------------------------------------------------------
// parse_frame_header — 12 bytes: bit7=media flag (0), bit6=config,
// bit5=key frame, bits restantes (61) = PTS; bytes8..12 = packet size BE.
// ---------------------------------------------------------------------------

#[test]
fn parses_frame_header_plain_frame() {
    // pts=12345, packet_size=6789, sem flags.
    let bytes: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0x39, 0x00, 0x00, 0x1A, 0x85,
    ];
    let header = parse_frame_header(&bytes).expect("deve reconhecer frame header");
    assert!(!header.config_packet);
    assert!(!header.key_frame);
    assert_eq!(header.pts, 12345);
    assert_eq!(header.packet_size, 6789);
}

#[test]
fn parses_frame_header_key_frame_config_packet() {
    // byte0 = 0b0110_0000 (config=1, key=1), mesmo pts/packet_size acima.
    let bytes: [u8; 12] = [
        0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0x39, 0x00, 0x00, 0x1A, 0x85,
    ];
    let header = parse_frame_header(&bytes).expect("deve reconhecer frame header");
    assert!(header.config_packet);
    assert!(header.key_frame);
    assert_eq!(header.pts, 12345);
}

#[test]
fn frame_header_masks_out_flag_bits_from_max_pts() {
    // PTS máximo (61 bits em 1) com key_frame ligado: os 3 bits de flag no
    // topo do byte 0 não podem vazar para o valor de PTS decodificado.
    let max_pts: u64 = (1u64 << 61) - 1;
    let flags: u64 = 0b001; // media=0, config=0, key=1
    let word = (flags << 61) | max_pts;
    let mut bytes = [0u8; 12];
    bytes[0..8].copy_from_slice(&word.to_be_bytes());
    bytes[8..12].copy_from_slice(&0u32.to_be_bytes());

    let header = parse_frame_header(&bytes).expect("deve reconhecer frame header");
    assert!(header.key_frame);
    assert!(!header.config_packet);
    assert_eq!(header.pts, max_pts);
}

#[test]
fn session_packet_bytes_are_not_a_frame_header() {
    let bytes: [u8; 12] = [
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x00, 0x04, 0x38,
    ];
    assert!(parse_frame_header(&bytes).is_none());
}

// ---------------------------------------------------------------------------
// Ids de codec — comparados como u32 big-endian do ASCII de 4 letras.
// ---------------------------------------------------------------------------

#[test]
fn codec_ids_match_documented_ascii_values() {
    assert_eq!(CODEC_ID_H264, u32::from_be_bytes(*b"h264"));
    assert_eq!(CODEC_ID_H265, u32::from_be_bytes(*b"h265"));
}

// ---------------------------------------------------------------------------
// parse_forward_port — `adb forward tcp:0 <remote>` imprime só o número da
// porta alocada no stdout.
// ---------------------------------------------------------------------------

#[test]
fn parses_forward_port_from_plain_output() {
    assert_eq!(parse_forward_port("42891\n"), Some(42891));
}

#[test]
fn parses_forward_port_without_trailing_newline() {
    assert_eq!(parse_forward_port("5037"), Some(5037));
}

#[test]
fn unparseable_forward_output_is_none() {
    assert_eq!(parse_forward_port(""), None);
    assert_eq!(
        parse_forward_port("error: no devices/emulators found"),
        None
    );
}
