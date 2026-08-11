//! T055 — Testes do receptor de framing binário da Captura RAW (US5):
//! `[u8 tag=0xD1][u32be metadata_len][metadata JSON][u64be dng_len][bytes DNG]`
//! (contracts/control-protocol.md §4). Cobre frame completo, frames parciais
//! (streaming) em todas as fronteiras do cabeçalho, frames corrompidos (tag
//! errada / metadata inválida) e gravação em disco com nome derivado do
//! timestamp.

use camlink_lib::raw_manager::{
    dng_filename, encode_frame, parse_frame, write_frame, FrameError, RawFrame, RawFrameMetadata,
};

fn sample_metadata() -> RawFrameMetadata {
    RawFrameMetadata {
        seq: 7,
        timestamp_ms: 1_785_800_000_123,
        width: 4032,
        height: 3024,
    }
}

// ---------------------------------------------------------------------------
// Frame completo
// ---------------------------------------------------------------------------

#[test]
fn parses_a_complete_frame_and_reports_bytes_consumed() {
    let metadata = sample_metadata();
    let dng = vec![0xAB; 128];
    let encoded = encode_frame(&metadata, &dng);

    let (frame, consumed) = parse_frame(&encoded)
        .expect("frame válido não deve dar erro")
        .expect("todos os bytes estão presentes, não deve pedir mais dados");

    assert_eq!(consumed, encoded.len());
    assert_eq!(frame.metadata, metadata);
    assert_eq!(frame.dng, dng);
}

#[test]
fn only_consumes_the_first_frame_when_buffer_has_several() {
    let meta_a = sample_metadata();
    let mut meta_b = sample_metadata();
    meta_b.seq = 8;

    let mut buf = encode_frame(&meta_a, &[1, 2, 3]);
    let frame_b = encode_frame(&meta_b, &[4, 5, 6, 7]);
    buf.extend_from_slice(&frame_b);

    let (frame, consumed) = parse_frame(&buf).unwrap().unwrap();
    assert_eq!(frame.metadata, meta_a);
    assert_eq!(frame.dng, vec![1, 2, 3]);

    // O restante do buffer, a partir de `consumed`, é exatamente o segundo frame.
    let (frame2, consumed2) = parse_frame(&buf[consumed..]).unwrap().unwrap();
    assert_eq!(frame2.metadata, meta_b);
    assert_eq!(consumed2, frame_b.len());
}

// ---------------------------------------------------------------------------
// Frames parciais (streaming) — nunca erro, sempre `Ok(None)`
// ---------------------------------------------------------------------------

#[test]
fn empty_buffer_asks_for_more_data() {
    assert_eq!(parse_frame(&[]).unwrap(), None);
}

#[test]
fn buffer_shorter_than_fixed_header_asks_for_more_data() {
    // Só tag + parte do metadata_len (precisa de 5 bytes pro cabeçalho fixo).
    let encoded = encode_frame(&sample_metadata(), &[1, 2, 3]);
    for cut in 0..5 {
        assert_eq!(
            parse_frame(&encoded[..cut]).unwrap(),
            None,
            "cortado em {cut} bytes deveria pedir mais dados"
        );
    }
}

#[test]
fn buffer_cut_mid_metadata_json_asks_for_more_data() {
    let encoded = encode_frame(&sample_metadata(), &[1, 2, 3]);
    // 5 bytes de cabeçalho + parte do JSON, mas não tudo.
    let cut = 5 + 3;
    assert_eq!(parse_frame(&encoded[..cut]).unwrap(), None);
}

#[test]
fn buffer_cut_mid_dng_len_field_asks_for_more_data() {
    let encoded = encode_frame(&sample_metadata(), &[1, 2, 3, 4, 5]);
    let metadata_json_len = serde_json::to_vec(&sample_metadata()).unwrap().len();
    let metadata_end = 5 + metadata_json_len;
    // Cabeçalho + metadata completos, mas só parte do u64be de dng_len.
    let cut = metadata_end + 3;
    assert_eq!(parse_frame(&encoded[..cut]).unwrap(), None);
}

#[test]
fn buffer_cut_mid_dng_payload_asks_for_more_data() {
    let dng = vec![9u8; 100];
    let encoded = encode_frame(&sample_metadata(), &dng);
    // Corta faltando os últimos 10 bytes do payload DNG.
    let cut = encoded.len() - 10;
    assert_eq!(parse_frame(&encoded[..cut]).unwrap(), None);
}

#[test]
fn frame_arriving_in_small_chunks_eventually_parses() {
    // Simula o socket entregando o frame em pedaços de 7 bytes.
    let metadata = sample_metadata();
    let dng = vec![0x42; 50];
    let encoded = encode_frame(&metadata, &dng);

    let mut received = Vec::new();
    let mut result = None;
    for chunk in encoded.chunks(7) {
        received.extend_from_slice(chunk);
        match parse_frame(&received) {
            Ok(Some((frame, consumed))) => {
                result = Some((frame, consumed));
                break;
            }
            Ok(None) => continue,
            Err(e) => panic!("não deveria falhar com dado parcial válido: {e}"),
        }
    }

    let (frame, consumed) = result.expect("deveria ter parseado ao juntar todos os pedaços");
    assert_eq!(consumed, encoded.len());
    assert_eq!(frame.metadata, metadata);
    assert_eq!(frame.dng, dng);
}

// ---------------------------------------------------------------------------
// Frames corrompidos — erro definitivo, não "esperar mais dados"
// ---------------------------------------------------------------------------

#[test]
fn wrong_tag_byte_is_rejected_even_with_full_buffer() {
    let mut encoded = encode_frame(&sample_metadata(), &[1, 2, 3]);
    encoded[0] = 0xFF;
    assert_eq!(parse_frame(&encoded), Err(FrameError::BadTag(0xFF)));
}

#[test]
fn wrong_tag_byte_is_rejected_immediately_even_with_short_buffer() {
    // Corrupção na tag é detectável assim que o primeiro byte chega — não
    // precisa esperar o cabeçalho inteiro pra reportar.
    let mut encoded = encode_frame(&sample_metadata(), &[1, 2, 3]);
    encoded[0] = 0x00;
    assert!(matches!(
        parse_frame(&encoded[..5]),
        Err(FrameError::BadTag(0x00))
    ));
}

#[test]
fn invalid_metadata_json_is_rejected() {
    let mut buf = vec![0xD1u8];
    let garbage = b"{not valid json";
    buf.extend_from_slice(&(garbage.len() as u32).to_be_bytes());
    buf.extend_from_slice(garbage);
    buf.extend_from_slice(&0u64.to_be_bytes());

    match parse_frame(&buf) {
        Err(FrameError::InvalidMetadata(_)) => {}
        other => panic!("esperava InvalidMetadata, veio {other:?}"),
    }
}

#[test]
fn metadata_missing_required_fields_is_rejected() {
    let mut buf = vec![0xD1u8];
    let incomplete = br#"{"seq": 1}"#; // falta timestamp_ms/width/height
    buf.extend_from_slice(&(incomplete.len() as u32).to_be_bytes());
    buf.extend_from_slice(incomplete);
    buf.extend_from_slice(&0u64.to_be_bytes());

    assert!(matches!(
        parse_frame(&buf),
        Err(FrameError::InvalidMetadata(_))
    ));
}

// ---------------------------------------------------------------------------
// Gravação em disco com nome derivado do timestamp
// ---------------------------------------------------------------------------

#[test]
fn dng_filename_embeds_sequence_and_timestamp() {
    let metadata = sample_metadata();
    let name = dng_filename(&metadata);
    assert!(name.starts_with("raw_000007_"), "{name}");
    assert!(name.contains("1785800000123"), "{name}");
    assert!(name.ends_with(".dng"), "{name}");
}

#[test]
fn different_sequences_never_collide_in_filename() {
    let mut a = sample_metadata();
    let mut b = sample_metadata();
    a.seq = 1;
    b.seq = 2;
    assert_ne!(dng_filename(&a), dng_filename(&b));
}

#[test]
fn write_frame_persists_exact_dng_bytes_at_expected_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let metadata = sample_metadata();
    let dng = vec![0x10, 0x20, 0x30, 0x40];
    let frame = RawFrame {
        metadata: metadata.clone(),
        dng: dng.clone(),
    };

    let path = write_frame(dir.path(), &frame).expect("gravação deveria funcionar");

    assert_eq!(path, dir.path().join(dng_filename(&metadata)));
    let written = std::fs::read(&path).unwrap();
    assert_eq!(written, dng);
}

#[test]
fn write_frame_to_nonexistent_dir_fails_instead_of_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nao-existe/subdir");
    let frame = RawFrame {
        metadata: sample_metadata(),
        dng: vec![1, 2, 3],
    };
    assert!(write_frame(&missing, &frame).is_err());
}
