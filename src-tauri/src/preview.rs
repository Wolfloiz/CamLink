//! T026 — Preview para fontes Android (FR-023): encode RGBA→JPEG,
//! compartilhado pelas duas plataformas. Os frames RGBA chegam pelo
//! `FrameSink` (Windows: pipeline de decode do socket scrcpy; Linux:
//! `run_preview_pipeline` lendo o lado de captura do device v4l2 — a
//! leitura YUYV manual via crate `v4l` foi removida porque o formato do
//! device é renegociado pelo scrcpy para YUV420P, o que quebrava a
//! conversão sempre, em silêncio).
//!
//! Frames de preview são descartáveis por natureza (FR-023): qualquer
//! falha aqui (formato inesperado, device ocupado) só significa "sem
//! preview desta vez", nunca deve derrubar o stream principal.

/// Codifica um frame RGBA em JPEG (qualidade fixa, suficiente para preview
/// em miniatura — não é o stream principal). Função pura, mesma em ambas
/// as plataformas.
pub fn encode_preview_jpeg(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(format!(
            "tamanho do frame ({}) não bate com {width}x{height} RGBA ({expected} esperado)",
            rgba.len()
        ));
    }
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for px in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }

    let mut jpeg_bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 70);
    encoder
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(jpeg_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_solid_color_frame_to_valid_jpeg() {
        let rgba = [200u8, 100, 50, 255].repeat(4 * 4);
        let jpeg = encode_preview_jpeg(&rgba, 4, 4).expect("encode deve suceder");
        // Todo JPEG começa com o marcador SOI (Start Of Image) 0xFFD8.
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn rejects_frame_with_wrong_length() {
        let rgba = vec![0u8; 10];
        assert!(encode_preview_jpeg(&rgba, 4, 4).is_err());
    }
}
