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

/// Largura alvo da miniatura de preview (FR-023). O frame full-res segue
/// intocado para a câmera virtual; só o preview trafega reduzido.
const PREVIEW_TARGET_WIDTH: u32 = 640;

/// Dimensões da miniatura de preview para uma resolução de captura — fonte
/// única de verdade entre o leitor ffmpeg (filtro `scale=PW:PH` explícito,
/// nunca `-2`: um arredondamento feito pelo ffmpeg divergente deste
/// desalinharia o `read_exact` do pipeline em silêncio) e o encoder JPEG.
/// Garantias: nunca 0, sempre par (PAR-safe para encoders/filtros).
pub fn preview_dimensions((w, h): (u32, u32)) -> (u32, u32) {
    fn even_floor(v: u32) -> u32 {
        (v & !1).max(2)
    }
    if w == 0 || h == 0 {
        return (2, 2);
    }
    if w <= PREVIEW_TARGET_WIDTH {
        return (even_floor(w), even_floor(h));
    }
    let pw = PREVIEW_TARGET_WIDTH;
    // Altura proporcional com arredondamento (u64 evita overflow em h*pw).
    let ph = ((h as u64 * pw as u64 + (w as u64 / 2)) / w as u64) as u32;
    (pw, even_floor(ph))
}

/// Reduz um frame RGBA para as dimensões de destino por nearest-neighbor —
/// qualidade suficiente para a miniatura de preview sem dependência nova nem
/// custo de filtro. Usada no Windows, onde os frames chegam ao sink em
/// full-res do decode; no Linux o leitor ffmpeg já entrega no tamanho da
/// miniatura. Função pura.
pub fn downsample_rgba(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Result<Vec<u8>, String> {
    let expected = src_w as usize * src_h as usize * 4;
    if src.len() != expected {
        return Err(format!(
            "tamanho da fonte ({}) não bate com {src_w}x{src_h} RGBA ({expected} esperado)",
            src.len()
        ));
    }
    if dst_w == 0 || dst_h == 0 {
        return Err(format!("dimensões de destino inválidas: {dst_w}x{dst_h}"));
    }
    let mut out = Vec::with_capacity(dst_w as usize * dst_h as usize * 4);
    for y in 0..dst_h as usize {
        let sy = y * src_h as usize / dst_h as usize;
        let row = sy * src_w as usize * 4;
        for x in 0..dst_w as usize {
            let sx = x * src_w as usize / dst_w as usize;
            let px = row + sx * 4;
            out.extend_from_slice(&src[px..px + 4]);
        }
    }
    Ok(out)
}

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
    fn preview_dimensions_downscales_1080p_to_640x360() {
        assert_eq!(preview_dimensions((1920, 1080)), (640, 360));
    }

    #[test]
    fn preview_dimensions_downscales_720p_to_640x360() {
        assert_eq!(preview_dimensions((1280, 720)), (640, 360));
    }

    #[test]
    fn preview_dimensions_keeps_source_at_or_below_target_width() {
        assert_eq!(preview_dimensions((640, 480)), (640, 480));
        assert_eq!(preview_dimensions((320, 240)), (320, 240));
    }

    #[test]
    fn preview_dimensions_handles_non_16_9_aspect() {
        assert_eq!(preview_dimensions((1440, 1080)), (640, 480));
    }

    #[test]
    fn preview_dimensions_forces_even_dimensions() {
        let (w, h) = preview_dimensions((639, 479));
        assert_eq!(w % 2, 0, "largura deve ser par");
        assert_eq!(h % 2, 0, "altura deve ser par");
        // Retrato 1080x1920 (rotação): altura proporcional 1138 → par.
        let (w, h) = preview_dimensions((1080, 1920));
        assert_eq!(w, 640);
        assert_eq!(h % 2, 0);
        assert!(h > 0);
    }

    #[test]
    fn preview_dimensions_never_returns_zero() {
        let (w, h) = preview_dimensions((0, 0));
        assert!(w > 0 && h > 0);
        // Extremo panorâmico: altura arredondaria para 0 sem o piso.
        let (w, h) = preview_dimensions((10_000, 4));
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn downsample_produces_target_dimensions() {
        let src = [0u8; 4 * 4 * 4];
        let out = downsample_rgba(&src, 4, 4, 2, 2).expect("downsample deve suceder");
        assert_eq!(out.len(), 2 * 2 * 4);
    }

    #[test]
    fn downsample_picks_nearest_neighbor_pixels() {
        // Fonte 4x4 em blocos 2x2 de cores distintas:
        //   A A B B        A = (10,0,0,255)   B = (20,0,0,255)
        //   A A B B
        //   C C D D        C = (30,0,0,255)   D = (40,0,0,255)
        //   C C D D
        let mut src = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4u32 {
            for x in 0..4u32 {
                let r = match (x < 2, y < 2) {
                    (true, true) => 10,
                    (false, true) => 20,
                    (true, false) => 30,
                    (false, false) => 40,
                };
                src.extend_from_slice(&[r, 0, 0, 255]);
            }
        }
        // 4x4 → 2x2: cada pixel destino cai no canto superior esquerdo do
        // bloco correspondente (mapeamento x*sw/dw).
        let out = downsample_rgba(&src, 4, 4, 2, 2).expect("downsample deve suceder");
        assert_eq!(out[0], 10, "dst(0,0) vem do bloco A");
        assert_eq!(out[4], 20, "dst(1,0) vem do bloco B");
        assert_eq!(out[8], 30, "dst(0,1) vem do bloco C");
        assert_eq!(out[12], 40, "dst(1,1) vem do bloco D");
    }

    #[test]
    fn downsample_rejects_inconsistent_source() {
        let src = [0u8; 10];
        assert!(downsample_rgba(&src, 4, 4, 2, 2).is_err());
        let ok = [0u8; 4 * 4 * 4];
        assert!(
            downsample_rgba(&ok, 4, 4, 0, 2).is_err(),
            "destino 0 é erro"
        );
    }

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
