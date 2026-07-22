//! T076 — Transform de orientação de frames RGBA (US2 / FR-016a).
//!
//! Função pura, sem I/O, compartilhada pelas duas plataformas: espelha
//! horizontalmente e/ou gira em passos de 90°. A ordem é fixa e documentada
//! no contrato dos testes (frame_transform_test.rs): espelha PRIMEIRO, na
//! orientação da fonte, depois gira no sentido horário.

use crate::model::Rotation;

/// Aplica `mirror` (horizontal) e `rotation` a um frame RGBA `width×height`.
/// Devolve o frame transformado e as dimensões resultantes — trocadas
/// (height, width) para 90°/270°.
///
/// Identidade (`Deg0`, sem mirror) copia o frame sem transformar; o caller
/// decide evitar a chamada nesse caso se quiser economizar a cópia.
pub fn apply(
    frame: &[u8],
    width: u32,
    height: u32,
    rotation: Rotation,
    mirror: bool,
) -> (Vec<u8>, u32, u32) {
    let (w, h) = (width as usize, height as usize);
    debug_assert_eq!(
        frame.len(),
        w * h * 4,
        "frame não bate com {width}x{height} RGBA"
    );

    let (out_w, out_h) = if rotation.swaps_dimensions() {
        (height, width)
    } else {
        (width, height)
    };

    let mut out = vec![0u8; frame.len()];
    for y in 0..h {
        for x in 0..w {
            // Espelho horizontal primeiro, na orientação da fonte.
            let sx = if mirror { w - 1 - x } else { x };
            let src = (y * w + sx) * 4;
            // Depois a rotação horária: destino do pixel lógico (x, y).
            let (dx, dy) = match rotation {
                Rotation::Deg0 => (x, y),
                Rotation::Deg90 => (h - 1 - y, x),
                Rotation::Deg180 => (w - 1 - x, h - 1 - y),
                Rotation::Deg270 => (y, w - 1 - x),
            };
            let dst = (dy * out_w as usize + dx) * 4;
            out[dst..dst + 4].copy_from_slice(&frame[src..src + 4]);
        }
    }
    (out, out_w, out_h)
}
