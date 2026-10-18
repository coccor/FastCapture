//! FastCapture's icon, drawn in code so no image file is needed: a blue rounded square with
//! white screenshot-frame corners and a dot. `build.rs` includes this file to embed an `.ico`
//! in the executable, and the tray draws its icon from the same pixels at the size Windows
//! asks for. Pure: no Windows calls.

/// The icon at `size` × `size` pixels as 32-bit BGRA, premultiplied alpha off, top row first.
pub fn pixels(size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut blue, mut white) = (0.0f32, 0.0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / s;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / s;
                    if in_rounded_square(px, py) {
                        if in_mark(px, py) {
                            white += 1.0;
                        } else {
                            blue += 1.0;
                        }
                    }
                }
            }
            let total = (SAMPLES * SAMPLES) as f32;
            let alpha = (blue + white) / total;
            if alpha == 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // #2563EB under white, by coverage.
            let mix = |base: f32| -> u8 {
                ((base * blue + 255.0 * white) / (blue + white)).round() as u8
            };
            out.extend_from_slice(&[
                mix(235.0),
                mix(99.0),
                mix(37.0),
                (alpha * 255.0).round() as u8,
            ]);
        }
    }
    out
}

/// The square inset by 1/16, with corners rounded at 22%.
fn in_rounded_square(x: f32, y: f32) -> bool {
    let (inset, radius) = (1.0 / 16.0, 0.22);
    let (low, high) = (inset + radius, 1.0 - inset - radius);
    if !(inset..=1.0 - inset).contains(&x) || !(inset..=1.0 - inset).contains(&y) {
        return false;
    }
    let dx = (low - x).max(x - high).max(0.0);
    let dy = (low - y).max(y - high).max(0.0);
    dx * dx + dy * dy <= radius * radius
}

/// Four L-shaped corners of a frame, and a dot in the middle.
fn in_mark(x: f32, y: f32) -> bool {
    let (edge, arm, thick) = (0.25, 0.17, 0.085);
    let near = |v: f32| v >= edge && v <= edge + thick;
    let far = |v: f32| v <= 1.0 - edge && v >= 1.0 - edge - thick;
    let along_low = |v: f32| v >= edge && v <= edge + arm;
    let along_high = |v: f32| v <= 1.0 - edge && v >= 1.0 - edge - arm;
    let corner =
        |hx: &dyn Fn(f32) -> bool,
         hy: &dyn Fn(f32) -> bool,
         ax: &dyn Fn(f32) -> bool,
         ay: &dyn Fn(f32) -> bool| { (hy(y) && ax(x)) || (hx(x) && ay(y)) };
    let frame = corner(&near, &near, &along_low, &along_low)
        || corner(&far, &near, &along_high, &along_low)
        || corner(&near, &far, &along_low, &along_high)
        || corner(&far, &far, &along_high, &along_high);
    let (dx, dy) = (x - 0.5, y - 0.5);
    frame || dx * dx + dy * dy <= 0.085 * 0.085
}

/// One icon image as an `.ico` stores it: a `BITMAPINFOHEADER` with double height, the pixels
/// bottom row first, then an all-clear AND mask.
pub fn bitmap(size: u32) -> Vec<u8> {
    let pixels = pixels(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    for value in [40u32, size, size * 2] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(size * size * 4 + mask_row * size).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for row in (0..size as usize).rev() {
        let width = size as usize * 4;
        out.extend_from_slice(&pixels[row * width..(row + 1) * width]);
    }
    out.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
    out
}

/// An `.ico` file with the icon at each of `sizes` (each under 256).
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|size| bitmap(*size)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, image) in sizes.iter().zip(&images) {
        out.extend_from_slice(&[*size as u8, *size as u8, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        out.extend_from_slice(&image);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_has_clear_corners_a_blue_body_and_a_white_mark() {
        let size = 32;
        let pixels = pixels(size);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        assert_eq!(at(0, 0)[3], 0, "outside the rounded corner");
        assert_eq!(at(16, 4), [235, 99, 37, 255], "blue body");
        assert_eq!(at(16, 16), [255, 255, 255, 255], "white dot");
        assert_eq!(at(9, 8)[0..3], [255, 255, 255], "a frame corner");
    }

    #[test]
    fn the_ico_file_has_a_directory_entry_per_size() {
        let ico = ico(&[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[22], 32);
        let first = bitmap(16).len();
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 38);
        assert_eq!(ico.len(), 38 + first + bitmap(32).len());
        assert_eq!(first, 40 + 16 * 16 * 4 + 4 * 16);
    }
}
