//! Drawing with GDI the way FastPad draws: flat fills, rounded shapes whose corner pixels are
//! blended by how much of each lies inside the arc (sampled 4×4), one focus ring, text and
//! glyphs, and painting into a memory bitmap first so nothing flickers.

// Device contexts and window handles are only passed on to Windows, which checks them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::theme::Palette;
use super::theme::metrics::{CONTROL_RADIUS, FOCUS_GAP, FOCUS_RING, scale};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLR_INVALID, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush,
    DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetPixel, HDC, HFONT, HGDIOBJ,
    PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, SetPixelV, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Segoe Fluent Icons (and MDL2) code points.
pub const GLYPH_MINIMIZE: &str = "\u{E921}";
pub const GLYPH_MAXIMIZE: &str = "\u{E922}";
pub const GLYPH_RESTORE: &str = "\u{E923}";
pub const GLYPH_CLOSE: &str = "\u{E8BB}";
pub const GLYPH_CHECK: &str = "\u{E73E}";
pub const GLYPH_CHEVRON_DOWN: &str = "\u{E70D}";
pub const GLYPH_CHEVRON_RIGHT: &str = "\u{E76C}";

/// `foreground` at `alpha`/255 over `background`, channel by channel.
pub const fn blend(foreground: u32, background: u32, alpha: u32) -> u32 {
    const fn channel(foreground: u32, background: u32, alpha: u32, shift: u32) -> u32 {
        let fg = (foreground >> shift) & 0xFF;
        let bg = (background >> shift) & 0xFF;
        ((fg * alpha + bg * (255 - alpha) + 127) / 255) << shift
    }
    channel(foreground, background, alpha, 0)
        | channel(foreground, background, alpha, 8)
        | channel(foreground, background, alpha, 16)
}

pub const fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

/// Fills `area` with `color`. An empty or inverted rect draws nothing.
pub fn fill(dc: HDC, area: RECT, color: u32) {
    if area.right <= area.left || area.bottom <= area.top {
        return;
    }
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &area, brush);
        DeleteObject(brush as HGDIOBJ);
    }
}

/// Which corners of a rect are rounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Corners(u8);

impl Corners {
    pub const NONE: Self = Self(0);
    pub const TOP_LEFT: Self = Self(1);
    pub const TOP_RIGHT: Self = Self(2);
    pub const BOTTOM_LEFT: Self = Self(4);
    pub const BOTTOM_RIGHT: Self = Self(8);
    pub const TOP: Self = Self(3);
    pub const BOTTOM: Self = Self(12);
    pub const ALL: Self = Self(15);

    const fn has(self, corner: Self) -> bool {
        self.0 & corner.0 != 0
    }
}

/// `at_96_dpi` at `dpi`, or 0 in high contrast, whose shapes are square and never blended.
pub fn radius(palette: &Palette, at_96_dpi: i32, dpi: u32) -> i32 {
    if palette.high_contrast {
        0
    } else {
        scale(at_96_dpi, dpi)
    }
}

/// How much of pixel (`x`, `y`) of a corner square lies inside its arc, 0 to 255. (0, 0) is the
/// outermost pixel; the arc's center is `radius` in from both edges. A radius of 0 has no corner.
pub fn coverage(x: i32, y: i32, radius: i32) -> u32 {
    if radius <= 0 {
        255
    } else {
        arc_coverage(x, y, radius, radius)
    }
}

/// How much of pixel (`x`, `y`) lies within `radius` of (`center`, `center`), sampled 4×4.
fn arc_coverage(x: i32, y: i32, center: i32, radius: i32) -> u32 {
    let center = 8 * center;
    let radius = 8 * radius;
    let mut inside = 0;
    for j in 0..4 {
        for i in 0..4 {
            let dx = 8 * x + 2 * i + 1 - center;
            let dy = 8 * y + 2 * j + 1 - center;
            if dx * dx + dy * dy <= radius * radius {
                inside += 1;
            }
        }
    }
    (inside * 255 + 8) / 16
}

/// How much of corner pixel (`x`, `y`) lies in a ring `width` wide just inside an arc of `radius`.
pub fn ring_coverage(x: i32, y: i32, radius: i32, width: i32) -> u32 {
    let outer = arc_coverage(x, y, radius, radius);
    let inner = if radius > width {
        arc_coverage(x, y, radius, radius - width)
    } else {
        0
    };
    outer.saturating_sub(inner)
}

/// The pixel at (`x`, `y`) of the corner square of `area` at `corner`.
fn corner_pixel(area: RECT, corner: Corners, x: i32, y: i32) -> (i32, i32) {
    let right = corner == Corners::TOP_RIGHT || corner == Corners::BOTTOM_RIGHT;
    let bottom = corner == Corners::BOTTOM_LEFT || corner == Corners::BOTTOM_RIGHT;
    (
        if right {
            area.right - 1 - x
        } else {
            area.left + x
        },
        if bottom {
            area.bottom - 1 - y
        } else {
            area.top + y
        },
    )
}

const EACH_CORNER: [Corners; 4] = [
    Corners::TOP_LEFT,
    Corners::TOP_RIGHT,
    Corners::BOTTOM_LEFT,
    Corners::BOTTOM_RIGHT,
];

/// Fills `area` with `color`, its `corners` rounded by `radius` (at most half the shorter side);
/// the corner pixels blend toward `behind`, the flat color already under the shape.
pub fn fill_rounded(dc: HDC, area: RECT, radius: i32, corners: Corners, color: u32, behind: u32) {
    let (width, height) = (area.right - area.left, area.bottom - area.top);
    if width <= 0 || height <= 0 {
        return;
    }
    let radius = radius.min(width / 2).min(height / 2).max(0);
    if radius == 0 || corners == Corners::NONE {
        fill(dc, area, color);
        return;
    }
    let inset = |corner| if corners.has(corner) { radius } else { 0 };
    let band = |top, bottom, left, right| {
        fill(
            dc,
            rect(area.left + left, top, area.right - right, bottom),
            color,
        )
    };
    band(
        area.top,
        area.top + radius,
        inset(Corners::TOP_LEFT),
        inset(Corners::TOP_RIGHT),
    );
    band(area.top + radius, area.bottom - radius, 0, 0);
    band(
        area.bottom - radius,
        area.bottom,
        inset(Corners::BOTTOM_LEFT),
        inset(Corners::BOTTOM_RIGHT),
    );
    for corner in EACH_CORNER
        .into_iter()
        .filter(|corner| corners.has(*corner))
    {
        for y in 0..radius {
            for x in 0..radius {
                let covered = coverage(x, y, radius);
                if covered > 0 {
                    let (px, py) = corner_pixel(area, corner, x, y);
                    unsafe { SetPixelV(dc, px, py, blend(color, behind, covered)) };
                }
            }
        }
    }
}

/// A rounded box with a 1 px `border`: the border's shape, then the fill inset by a pixel with a
/// radius a pixel smaller, so the border keeps its width around the corners.
pub fn fill_bordered(dc: HDC, area: RECT, radius: i32, color: u32, border: u32, behind: u32) {
    fill_rounded(dc, area, radius, Corners::ALL, border, behind);
    let inner = rect(area.left + 1, area.top + 1, area.right - 1, area.bottom - 1);
    fill_rounded(dc, inner, (radius - 1).max(0), Corners::ALL, color, border);
}

/// A ring `width` wide just inside `area`, rounded by `radius`. Its corner pixels blend over
/// what is already there, so it sits on any background.
pub fn stroke_ring(dc: HDC, area: RECT, radius: i32, width: i32, color: u32) {
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w <= 0 || h <= 0 || width <= 0 {
        return;
    }
    let radius = radius.min(w / 2).min(h / 2).max(0);
    let width = width
        .min(w / 2)
        .min(h / 2)
        .max(1)
        .min(if radius > 0 { radius } else { i32::MAX });
    let strip = |left, top, right, bottom| fill(dc, rect(left, top, right, bottom), color);
    strip(
        area.left + radius,
        area.top,
        area.right - radius,
        area.top + width,
    );
    strip(
        area.left + radius,
        area.bottom - width,
        area.right - radius,
        area.bottom,
    );
    strip(
        area.left,
        area.top + radius,
        area.left + width,
        area.bottom - radius,
    );
    strip(
        area.right - width,
        area.top + radius,
        area.right,
        area.bottom - radius,
    );
    for corner in EACH_CORNER.into_iter().filter(|_| radius > 0) {
        for y in 0..radius {
            for x in 0..radius {
                let covered = ring_coverage(x, y, radius, width);
                if covered == 0 {
                    continue;
                }
                let (px, py) = corner_pixel(area, corner, x, y);
                let under = unsafe { GetPixel(dc, px, py) };
                if under != CLR_INVALID {
                    unsafe { SetPixelV(dc, px, py, blend(color, under, covered)) };
                }
            }
        }
    }
}

/// The keyboard focus ring around `control`: 2 px of the accent, its outer edge 1 px inside the
/// control's rect, rounded as controls are.
pub fn focus_ring(dc: HDC, control: RECT, palette: &Palette, dpi: u32) {
    let gap = scale(FOCUS_GAP, dpi);
    stroke_ring(
        dc,
        rect(
            control.left + gap,
            control.top + gap,
            control.right - gap,
            control.bottom - gap,
        ),
        radius(palette, CONTROL_RADIUS, dpi),
        scale(FOCUS_RING, dpi),
        palette.accent,
    );
}

/// How text is placed in its rect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Align {
    /// One line, left, centered vertically, cut with "…" when too long.
    Left,
    /// One line, centered both ways.
    Center,
}

/// Draws `text` in `font` and `color` within `area`, with no background.
pub fn text(dc: HDC, text: &str, area: RECT, font: HFONT, color: u32, align: Align) {
    let format = DT_SINGLELINE
        | DT_VCENTER
        | DT_NOPREFIX
        | match align {
            Align::Left => DT_LEFT | DT_END_ELLIPSIS,
            Align::Center => DT_CENTER,
        };
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut area = area;
    unsafe {
        let previous = SelectObject(dc, font as HGDIOBJ);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        DrawTextW(dc, wide.as_ptr(), wide.len() as i32, &mut area, format);
        SelectObject(dc, previous);
    }
}

/// How wide `text` is in `font`, in pixels.
pub fn text_width(dc: HDC, text: &str, font: HFONT) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut area = rect(0, 0, 0, 0);
    unsafe {
        let previous = SelectObject(dc, font as HGDIOBJ);
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut area,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, previous);
    }
    area.right - area.left
}

/// Paints `window` through a memory bitmap the size of its client area: `draw` gets the bitmap's
/// DC and the client rect, and the result is copied to the screen in one go. Call it for
/// `WM_PAINT`, and answer `WM_ERASEBKGND` with 1.
pub fn buffered(window: HWND, draw: impl FnOnce(HDC, RECT)) {
    let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let dc = unsafe { BeginPaint(window, &mut paint) };
    if dc.is_null() {
        return;
    }
    let mut client = rect(0, 0, 0, 0);
    unsafe { GetClientRect(window, &mut client) };
    in_memory(dc, client, draw);
    unsafe { EndPaint(window, &paint) };
}

/// Draws into a memory bitmap the size of `area` and copies it onto `dc`; draws onto `dc` itself
/// when no bitmap can be made.
pub fn in_memory(dc: HDC, area: RECT, draw: impl FnOnce(HDC, RECT)) {
    let (width, height) = (area.right - area.left, area.bottom - area.top);
    if width <= 0 || height <= 0 {
        return;
    }
    let memory = unsafe { CreateCompatibleDC(dc) };
    let bitmap = if memory.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { CreateCompatibleBitmap(dc, width, height) }
    };
    if bitmap.is_null() {
        draw(dc, area);
    } else {
        unsafe {
            let previous = SelectObject(memory, bitmap as HGDIOBJ);
            draw(memory, rect(0, 0, width, height));
            BitBlt(
                dc, area.left, area.top, width, height, memory, 0, 0, SRCCOPY,
            );
            SelectObject(memory, previous);
            DeleteObject(bitmap as HGDIOBJ);
        }
    }
    if !memory.is_null() {
        unsafe { DeleteDC(memory) };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ui::theme::{DARK, LIGHT};
    use windows_sys::Win32::Graphics::Gdi::{GetDC, ReleaseDC};

    /// A memory DC with a `width` × `height` bitmap, for drawing in tests and reading pixels back.
    pub(crate) struct Canvas {
        pub(crate) dc: HDC,
        bitmap: HGDIOBJ,
        previous: HGDIOBJ,
    }

    impl Canvas {
        pub(crate) fn new(width: i32, height: i32) -> Self {
            unsafe {
                let screen = GetDC(std::ptr::null_mut());
                let dc = CreateCompatibleDC(screen);
                let bitmap = CreateCompatibleBitmap(screen, width, height) as HGDIOBJ;
                ReleaseDC(std::ptr::null_mut(), screen);
                let previous = SelectObject(dc, bitmap);
                Self {
                    dc,
                    bitmap,
                    previous,
                }
            }
        }

        pub(crate) fn pixel(&self, x: i32, y: i32) -> u32 {
            unsafe { GetPixel(self.dc, x, y) }
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.previous);
                DeleteObject(self.bitmap);
                DeleteDC(self.dc);
            }
        }
    }

    #[test]
    fn corner_coverage_runs_from_outside_to_inside() {
        assert_eq!(coverage(0, 0, 0), 255, "no radius, no corner");
        assert_eq!(
            coverage(0, 0, 4),
            0,
            "the outermost pixel is outside a 4 px arc"
        );
        assert_eq!(coverage(3, 3, 4), 255, "the innermost pixel is inside");
        let edge = coverage(1, 0, 4);
        assert!(edge > 0 && edge < 255, "{edge}");
        assert!(coverage(0, 3, 4) >= coverage(0, 2, 4));
        assert_eq!(ring_coverage(3, 3, 4, 2), 0, "inside the ring's hole");
        assert!(ring_coverage(1, 1, 4, 2) > 0);
        assert_eq!(blend(0x00FF_FFFF, 0, 255), 0x00FF_FFFF);
        assert_eq!(blend(0x00FF_FFFF, 0, 0), 0);
        assert_eq!(blend(0x00FF_FFFF, 0, 128), 0x0080_8080);
    }

    #[test]
    fn a_rounded_fill_leaves_its_corners_to_what_is_behind() {
        let canvas = Canvas::new(40, 20);
        fill(canvas.dc, rect(0, 0, 40, 20), DARK.strip_background);
        fill_rounded(
            canvas.dc,
            rect(0, 0, 40, 20),
            4,
            Corners::ALL,
            DARK.accent,
            DARK.strip_background,
        );
        assert_eq!(canvas.pixel(0, 0), DARK.strip_background, "the corner");
        assert_eq!(canvas.pixel(20, 10), DARK.accent, "the middle");
        assert_eq!(canvas.pixel(20, 0), DARK.accent, "a straight edge");
        let blended = canvas.pixel(1, 0);
        assert!(blended != DARK.accent && blended != DARK.strip_background);

        let square = Canvas::new(10, 10);
        fill_rounded(
            square.dc,
            rect(0, 0, 10, 10),
            0,
            Corners::ALL,
            LIGHT.accent,
            0,
        );
        assert_eq!(
            square.pixel(0, 0),
            LIGHT.accent,
            "high contrast's square corners"
        );
    }

    #[test]
    fn a_bordered_box_and_the_focus_ring_keep_their_width() {
        let canvas = Canvas::new(60, 30);
        fill(canvas.dc, rect(0, 0, 60, 30), LIGHT.strip_background);
        fill_bordered(
            canvas.dc,
            rect(0, 0, 60, 30),
            4,
            LIGHT.content_background,
            LIGHT.stroke,
            LIGHT.strip_background,
        );
        assert_eq!(canvas.pixel(30, 0), LIGHT.stroke);
        assert_eq!(canvas.pixel(30, 1), LIGHT.content_background);
        assert_eq!(canvas.pixel(0, 15), LIGHT.stroke);

        focus_ring(canvas.dc, rect(0, 0, 60, 30), &LIGHT, 96);
        assert_eq!(canvas.pixel(30, 0), LIGHT.stroke, "the 1 px gap");
        assert_eq!(canvas.pixel(30, 1), LIGHT.accent);
        assert_eq!(canvas.pixel(30, 2), LIGHT.accent);
        assert_eq!(canvas.pixel(30, 3), LIGHT.content_background);
        assert_eq!(
            canvas.pixel(30, 15),
            LIGHT.content_background,
            "only a ring"
        );
    }

    #[test]
    fn text_is_drawn_in_its_color() {
        let canvas = Canvas::new(80, 24);
        fill(canvas.dc, rect(0, 0, 80, 24), DARK.content_background);
        let font = crate::ui::theme::ramp::font(crate::ui::theme::ramp::TextStyle::BodyBold, 96);
        text(
            canvas.dc,
            "WWWW",
            rect(0, 0, 80, 24),
            font,
            DARK.content_foreground,
            Align::Left,
        );
        let width = text_width(canvas.dc, "WWWW", font);
        unsafe { DeleteObject(font as HGDIOBJ) };
        assert!(width > 20, "{width}");
        let inked = (0..80)
            .flat_map(|x| (0..24).map(move |y| (x, y)))
            .filter(|(x, y)| canvas.pixel(*x, *y) != DARK.content_background)
            .count();
        assert!(inked > 20, "{inked} pixels of text");
    }
}
