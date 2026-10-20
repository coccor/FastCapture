//! WCAG 2.x contrast, for checking the palettes. Colors are Windows `COLORREF`s (`0x00BBGGRR`).

/// WCAG relative luminance of a `COLORREF`.
pub fn luminance(color: u32) -> f64 {
    let channel = |shift: u32| {
        let value = f64::from((color >> shift) & 0xFF) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(0) + 0.7152 * channel(8) + 0.0722 * channel(16)
}

/// The contrast ratio of two colors, from 1:1 (the same) to 21:1 (black on white), either way
/// round.
pub fn ratio(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::ratio;

    #[test]
    fn black_on_white_is_21_to_1_a_color_on_itself_1_to_1_and_order_doesnt_matter() {
        assert!((ratio(0x0000_0000, 0x00FF_FFFF) - 21.0).abs() < 1e-9);
        assert!((ratio(0x0080_4020, 0x0080_4020) - 1.0).abs() < 1e-9);
        assert!((ratio(0x0012_3456, 0x00AB_CDEF) - ratio(0x00AB_CDEF, 0x0012_3456)).abs() < 1e-12);
    }
}
