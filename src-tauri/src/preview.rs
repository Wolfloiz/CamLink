//! T026 — Preview 1 fps para fontes Android (FR-023): encode RGBA→JPEG
//! (compartilhado pelas duas plataformas) e a leitura de volta do device
//! virtual no Linux (`crate v4l`; no Windows a Spike C reaproveita os
//! frames que já passam pelo pipeline de decode — ver `lib.rs`
//! `start_stream`, sem precisar ler o device de volta).
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

/// Converte um frame YUYV422 (formato negociado pelo ffmpeg em
/// `virtualcam::v4l2`, `-pix_fmt yuyv422`) para RGBA — usado só pela
/// leitura de preview no Linux, que lê de volta o que o ffmpeg escreveu no
/// device (o pipeline principal nunca decodifica no lado do CamLink no
/// Linux: quem escreve YUYV422 é o próprio ffmpeg spawnado por
/// `virtualcam::v4l2`). Conversão BT.601 padrão.
pub fn yuyv422_to_rgba(yuyv: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected = width as usize * height as usize * 2;
    if yuyv.len() != expected {
        return Err(format!(
            "tamanho do frame YUYV422 ({}) não bate com {width}x{height} ({expected} esperado)",
            yuyv.len()
        ));
    }
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for pair in yuyv.chunks_exact(4) {
        let (y0, u, y1, v) = (
            pair[0] as f32,
            pair[1] as f32,
            pair[2] as f32,
            pair[3] as f32,
        );
        for y in [y0, y1] {
            let c = y - 16.0;
            let d = u - 128.0;
            let e = v - 128.0;
            let r = (1.164 * c + 1.596 * e).round().clamp(0.0, 255.0) as u8;
            let g = (1.164 * c - 0.392 * d - 0.813 * e)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (1.164 * c + 2.017 * d).round().clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Ok(rgba)
}

/// Leitura de preview no Linux: captura um único frame do device v4l2
/// (formato YUYV422, negociado pelo ffmpeg em `virtualcam::v4l2`) e
/// devolve como RGBA. Chamado a ~1 fps por quem orquestra (`lib.rs`), numa
/// task própria via `spawn_blocking` (a API do crate `v4l` é bloqueante).
///
/// **Não compilado/testado nesta sessão** (ambiente de desenvolvimento é
/// Windows; este módulo só entra no build no Linux via `#[cfg]`) — precisa
/// de validação em CI/dev Linux antes de ser considerado confiável, mesma
/// ressalva de `virtualcam::v4l2` (T022).
#[cfg(target_os = "linux")]
pub fn capture_one_frame_linux(device_path: &str) -> Result<(Vec<u8>, u32, u32), String> {
    use v4l::io::traits::CaptureStream;
    use v4l::prelude::*;
    use v4l::video::Capture;

    let dev = Device::with_path(device_path).map_err(|e| e.to_string())?;
    let format = Capture::format(&dev).map_err(|e| e.to_string())?;
    let (width, height) = (format.width, format.height);

    let mut stream = MmapStream::with_buffers(&dev, v4l::buffer::Type::VideoCapture, 2)
        .map_err(|e| e.to_string())?;
    let (buf, _meta) = stream.next().map_err(|e| e.to_string())?;
    let rgba = yuyv422_to_rgba(buf, width, height)?;
    Ok((rgba, width, height))
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

    #[test]
    fn yuyv_black_converts_to_rgba_black() {
        // Y=16 (preto no range limitado), U=V=128 (neutro) → RGB ~(0,0,0).
        let yuyv = vec![16u8, 128, 16, 128];
        let rgba = yuyv422_to_rgba(&yuyv, 2, 1).expect("convert deve suceder");
        assert_eq!(rgba, vec![0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn yuyv_white_converts_to_rgba_white() {
        // Y=235 (branco no range limitado), U=V=128 (neutro) → RGB ~(255,255,255).
        let yuyv = vec![235u8, 128, 235, 128];
        let rgba = yuyv422_to_rgba(&yuyv, 2, 1).expect("convert deve suceder");
        assert_eq!(rgba, vec![255, 255, 255, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn yuyv_rejects_wrong_length() {
        assert!(yuyv422_to_rgba(&[0u8; 3], 2, 1).is_err());
    }
}
