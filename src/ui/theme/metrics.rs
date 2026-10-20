//! Sizes FastCapture's windows share, FastPad's, in pixels at 96 DPI, and the one `scale` that
//! turns them into device pixels.

/// `value` (pixels at 96 DPI) at `dpi`, rounded half up. A DPI of 0, which Windows gives for a
/// bad handle, counts as 96.
pub const fn scale(value: i32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { 96 } else { dpi };
    ((value as i64 * dpi as i64 + 48) / 96) as i32
}

/// Everything is placed on this grid.
pub const GRID: i32 = 4;
/// The corner radius of fields, rows, buttons and cards.
pub const CONTROL_RADIUS: i32 = 4;
/// The radius of a small popup's corners.
pub const POPUP_RADIUS: i32 = 8;
/// The keyboard-focus ring's stroke, and its gap outside what it rings.
pub const FOCUS_RING: i32 = 2;
pub const FOCUS_GAP: i32 = 1;
/// How far a list row's hover and selection fill is inset from the list's sides and from the
/// rows around it.
pub const ROW_INSET_X: i32 = 4;
pub const ROW_INSET_Y: i32 = 1;
/// The accent bar on a selected row.
pub const SELECTION_BAR_WIDTH: i32 = 3;
pub const SELECTION_BAR_HEIGHT: i32 = 16;
/// A list row.
pub const ROW: i32 = 26;
/// The title strip, and a caption button's width (its height is the strip's).
pub const TITLE_STRIP: i32 = 40;
pub const CAPTION_BUTTON: i32 = 46;
/// A caption glyph's size.
pub const CAPTION_GLYPH: i32 = 10;
/// The status line.
pub const STATUS_BAR: i32 = 22;
/// A dialog's padding, a control's height, a button's width, and the footer that holds the
/// buttons.
pub const DIALOG_PADDING: i32 = 20;
pub const CONTROL_HEIGHT: i32 = 26;
pub const BUTTON_WIDTH: i32 = 88;
pub const FOOTER: i32 = 56;
/// A toggle switch.
pub const TOGGLE_WIDTH: i32 = 40;
pub const TOGGLE_HEIGHT: i32 = 20;
/// An icon glyph.
pub const ICON: i32 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rounds_half_up_and_treats_0_as_96() {
        assert_eq!(scale(26, 96), 26);
        assert_eq!(scale(12, 120), 15);
        assert_eq!(scale(26, 144), 39);
        assert_eq!(scale(1, 144), 2);
        assert_eq!(scale(520, 192), 1040);
        assert_eq!(scale(26, 0), 26);
    }

    #[test]
    fn layout_sizes_are_on_the_grid() {
        // A row, a control, a caption button and the status line are FastPad's own sizes,
        // which sit off the grid.
        for (name, size) in [
            ("title strip", TITLE_STRIP),
            ("dialog padding", DIALOG_PADDING),
            ("button", BUTTON_WIDTH),
            ("footer", FOOTER),
            ("toggle", TOGGLE_WIDTH),
            ("toggle height", TOGGLE_HEIGHT),
            ("control radius", CONTROL_RADIUS),
            ("popup radius", POPUP_RADIUS),
            ("icon", ICON),
        ] {
            assert_eq!(size % GRID, 0, "{name}: {size}");
        }
    }
}
