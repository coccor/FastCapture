//! FastCapture's colors, after FastPad's so the two read as one family: one palette of roles,
//! FastPad's light or dark values, or Windows' own colors in high contrast; which one the `theme`
//! setting and Windows choose; and telling the open windows when that changes.

pub mod contrast;
pub mod faces;
pub mod metrics;
pub mod ramp;

pub use crate::config::settings::ThemePreference;
pub use metrics::scale;

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontW, DEFAULT_CHARSET, FF_DONTCARE,
    FW_NORMAL, FW_SEMIBOLD, GetSysColor, HFONT, OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, PostMessageW, RegisterWindowMessageW,
};

/// A `COLORREF` from RGB.
pub const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | (green as u32) << 8 | (blue as u32) << 16
}

/// Every color a FastCapture window draws with, by role. The roles and values are FastPad's;
/// FastPad's `editor_*` roles are `content_*` here, the field and list surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    /// The title strip, the status line and the window behind the content.
    pub strip_background: u32,
    /// Glyphs and text on the strip.
    pub strip_foreground: u32,
    /// Fields, lists and the preview.
    pub content_background: u32,
    pub content_foreground: u32,
    /// Secondary text, on the strip or the content.
    pub muted_foreground: u32,
    pub hover_background: u32,
    /// Text on a hover fill, and on a selected row (FastPad's dark text is too dim on its
    /// selection blue).
    pub hover_foreground: u32,
    pub pressed_background: u32,
    pub close_hover_background: u32,
    pub close_hover_foreground: u32,
    pub close_pressed_background: u32,
    pub selection_background: u32,
    /// A selection in a list that doesn't have the focus.
    pub inactive_selection_background: u32,
    /// Selected text's color: `None` keeps the text's own, which the themed palettes want; high
    /// contrast must use the system pair.
    pub selection_foreground: Option<u32>,
    /// The focus ring, a selected row's bar and a primary button's fill.
    pub accent: u32,
    /// Text and glyphs on the accent.
    pub on_accent: u32,
    /// 1 px separators and control borders.
    pub stroke: u32,
    /// Disabled text and glyphs; exempt from the contrast rules, as disabled controls are.
    pub disabled_foreground: u32,
    pub error_foreground: u32,
    pub warning_foreground: u32,
    pub success_foreground: u32,
    /// Whether the window frame and scroll bars should ask for Windows' dark styling.
    pub dark_frame: bool,
    /// Windows' high-contrast colors: only system color pairs are drawn, never a blend, and
    /// corners are square.
    pub high_contrast: bool,
}

const CLOSE_HOVER: u32 = rgb(0xC4, 0x2B, 0x1C);
const CLOSE_PRESSED: u32 = rgb(0x94, 0x1F, 0x15);
const WHITE: u32 = rgb(255, 255, 255);

pub const LIGHT: Palette = Palette {
    strip_background: rgb(243, 243, 243),
    strip_foreground: rgb(32, 32, 32),
    content_background: WHITE,
    content_foreground: rgb(0, 0, 0),
    muted_foreground: rgb(96, 96, 96),
    hover_background: rgb(224, 224, 224),
    hover_foreground: rgb(0, 0, 0),
    pressed_background: rgb(204, 204, 204),
    close_hover_background: CLOSE_HOVER,
    close_hover_foreground: WHITE,
    close_pressed_background: CLOSE_PRESSED,
    selection_background: rgb(158, 204, 255),
    inactive_selection_background: rgb(208, 218, 232),
    selection_foreground: None,
    accent: rgb(0, 95, 184),
    on_accent: WHITE,
    stroke: rgb(204, 204, 204),
    disabled_foreground: rgb(160, 160, 160),
    error_foreground: rgb(0xA1, 0x26, 0x0D),
    warning_foreground: rgb(157, 93, 0),
    success_foreground: rgb(15, 123, 15),
    dark_frame: false,
    high_contrast: false,
};

pub const DARK: Palette = Palette {
    strip_background: rgb(37, 37, 38),
    strip_foreground: rgb(212, 212, 212),
    content_background: rgb(30, 30, 30),
    content_foreground: rgb(212, 212, 212),
    muted_foreground: rgb(150, 150, 150),
    hover_background: rgb(55, 55, 58),
    hover_foreground: rgb(240, 240, 240),
    pressed_background: rgb(72, 72, 76),
    close_hover_background: CLOSE_HOVER,
    close_hover_foreground: WHITE,
    close_pressed_background: CLOSE_PRESSED,
    selection_background: rgb(48, 100, 160),
    inactive_selection_background: rgb(66, 76, 92),
    selection_foreground: None,
    accent: rgb(96, 205, 255),
    on_accent: rgb(0, 0, 0),
    stroke: rgb(72, 72, 76),
    disabled_foreground: rgb(110, 110, 110),
    error_foreground: rgb(0xF4, 0x87, 0x71),
    warning_foreground: rgb(252, 225, 0),
    success_foreground: rgb(108, 203, 95),
    dark_frame: true,
    high_contrast: false,
};

impl Palette {
    /// Windows' high-contrast colors, read now.
    pub fn high_contrast() -> Self {
        let color = |index| unsafe { GetSysColor(index) };
        let window = color(COLOR_WINDOW);
        let text = color(COLOR_WINDOWTEXT);
        let highlight = color(COLOR_HIGHLIGHT);
        let highlight_text = color(COLOR_HIGHLIGHTTEXT);
        Self {
            strip_background: color(COLOR_BTNFACE),
            strip_foreground: color(COLOR_BTNTEXT),
            content_background: window,
            content_foreground: text,
            muted_foreground: color(COLOR_BTNTEXT),
            hover_background: highlight,
            hover_foreground: highlight_text,
            pressed_background: highlight,
            close_hover_background: highlight,
            close_hover_foreground: highlight_text,
            close_pressed_background: highlight,
            selection_background: highlight,
            inactive_selection_background: highlight,
            selection_foreground: Some(highlight_text),
            accent: highlight,
            on_accent: highlight_text,
            stroke: text,
            disabled_foreground: color(COLOR_GRAYTEXT),
            error_foreground: text,
            warning_foreground: text,
            success_foreground: text,
            dark_frame: false,
            high_contrast: true,
        }
    }
}

/// What Windows says: dark apps, and high contrast.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct System {
    pub dark: bool,
    pub high_contrast: bool,
}

/// The palette for `preference` when Windows is as `system` says. High contrast wins over any
/// theme.
pub fn resolve(preference: ThemePreference, system: System) -> Palette {
    if system.high_contrast {
        return Palette::high_contrast();
    }
    let dark = match preference {
        ThemePreference::System => system.dark,
        ThemePreference::Light => false,
        ThemePreference::Dark => true,
    };
    if dark { DARK } else { LIGHT }
}

/// Tests never read the machine's settings: they would depend on it.
#[cfg(test)]
fn read_system() -> System {
    System::default()
}

/// Dark apps from `AppsUseLightTheme` (light when it can't be read), and high contrast from
/// `SPI_GETHIGHCONTRAST`.
#[cfg(not(test))]
fn read_system() -> System {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &mut size,
        )
    };
    let mut contrast: HIGHCONTRASTW = unsafe { std::mem::zeroed() };
    contrast.cbSize = size_of::<HIGHCONTRASTW>() as u32;
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&raw mut contrast).cast(),
            0,
        )
    };
    System {
        dark: status == 0 && value == 0,
        high_contrast: read != 0 && contrast.dwFlags & HCF_HIGHCONTRASTON != 0,
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The setting and what Windows last said; Windows is read the first time it is needed.
struct State {
    preference: ThemePreference,
    system: Option<System>,
}

static STATE: Mutex<State> = Mutex::new(State {
    preference: ThemePreference::System,
    system: None,
});

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut state)
}

fn palette_of(state: &mut State) -> Palette {
    let system = *state.system.get_or_insert_with(read_system);
    resolve(state.preference, system)
}

/// The palette windows should draw with now.
pub fn current() -> Palette {
    with_state(palette_of)
}

/// Sets the `theme` setting; true when that changes the palette.
pub fn set_preference(preference: ThemePreference) -> bool {
    with_state(|state| {
        let before = palette_of(state);
        state.preference = preference;
        palette_of(state) != before
    })
}

/// Reads Windows again (dark apps, high contrast, text size); true when the palette or the text
/// size changed.
pub fn refresh() -> bool {
    let text = ramp::refresh();
    let palette = with_state(|state| {
        let before = palette_of(state);
        state.system = Some(read_system());
        palette_of(state) != before
    });
    text || palette
}

/// Sets what Windows says, for tests.
#[cfg(test)]
pub fn set_system_for_test(system: System) {
    with_state(|state| state.system = Some(system));
}

/// The message every FastCapture window gets when the theme changed: it reads `current()` again
/// and repaints. Registered, so it can't collide with a window's own `WM_APP` messages.
pub fn changed_message() -> u32 {
    static MESSAGE: AtomicU32 = AtomicU32::new(0);
    match MESSAGE.load(Ordering::Relaxed) {
        0 => {
            let message =
                unsafe { RegisterWindowMessageW(wide("FastCapture.ThemeChanged").as_ptr()) };
            MESSAGE.store(message, Ordering::Relaxed);
            message
        }
        message => message,
    }
}

/// Posts `changed_message()` to every top-level window of this thread, hidden ones too, so a
/// window that isn't showing catches up before it is shown again.
pub fn tell_windows() {
    unsafe extern "system" fn post(window: HWND, message: LPARAM) -> i32 {
        unsafe { PostMessageW(window, message as u32, 0, 0) };
        1
    }
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(post),
            changed_message() as LPARAM,
        )
    };
}

/// Segoe UI at `points` and `dpi`, semibold or not. The caller deletes it.
pub fn font(points: i32, semibold: bool, dpi: u32) -> HFONT {
    let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let height = -((points * dpi as i32 + 36) / 72);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            if semibold { FW_SEMIBOLD } else { FW_NORMAL } as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            0,
            CLEARTYPE_QUALITY as u32,
            FF_DONTCARE as u32,
            face.as_ptr(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contrast::ratio;

    #[test]
    fn the_palettes_are_fastpads() {
        assert_eq!(rgb(0x12, 0x34, 0x56), 0x0056_3412);
        assert_eq!(
            (
                LIGHT.strip_background,
                LIGHT.content_background,
                LIGHT.content_foreground
            ),
            (rgb(243, 243, 243), rgb(255, 255, 255), rgb(0, 0, 0))
        );
        assert_eq!(
            (
                DARK.strip_background,
                DARK.content_background,
                DARK.content_foreground
            ),
            (rgb(37, 37, 38), rgb(30, 30, 30), rgb(212, 212, 212))
        );
        assert_eq!(
            (LIGHT.accent, DARK.accent),
            (rgb(0, 95, 184), rgb(96, 205, 255))
        );
        assert_eq!(
            (LIGHT.selection_background, DARK.selection_background),
            (rgb(158, 204, 255), rgb(48, 100, 160))
        );
        for palette in [LIGHT, DARK] {
            assert_eq!(
                palette.stroke, palette.pressed_background,
                "borders are the pressed tone"
            );
            assert_eq!(palette.close_hover_background, rgb(0xC4, 0x2B, 0x1C));
        }
        const { assert!(DARK.dark_frame && !LIGHT.dark_frame) };
    }

    #[test]
    fn every_palette_keeps_text_legible() {
        for (name, p) in [("light", LIGHT), ("dark", DARK)] {
            for (what, foreground, background, minimum) in [
                (
                    "content text",
                    p.content_foreground,
                    p.content_background,
                    4.5,
                ),
                ("strip text", p.strip_foreground, p.strip_background, 4.5),
                (
                    "muted text on the strip",
                    p.muted_foreground,
                    p.strip_background,
                    4.5,
                ),
                (
                    "muted text on the content",
                    p.muted_foreground,
                    p.content_background,
                    4.5,
                ),
                ("hover text", p.hover_foreground, p.hover_background, 4.5),
                ("error text", p.error_foreground, p.content_background, 4.5),
                (
                    "warning text",
                    p.warning_foreground,
                    p.content_background,
                    4.5,
                ),
                (
                    "success text",
                    p.success_foreground,
                    p.content_background,
                    4.5,
                ),
                ("text on the accent", p.on_accent, p.accent, 4.5),
                (
                    "text on a selection",
                    p.hover_foreground,
                    p.selection_background,
                    4.5,
                ),
                (
                    "the accent on the content",
                    p.accent,
                    p.content_background,
                    3.0,
                ),
                ("the accent on the strip", p.accent, p.strip_background, 3.0),
                (
                    "the close glyph",
                    p.close_hover_foreground,
                    p.close_hover_background,
                    4.5,
                ),
            ] {
                let got = ratio(foreground, background);
                assert!(
                    got >= minimum,
                    "{name}: {what} is {got:.2}:1, under {minimum}:1"
                );
            }
        }
    }

    #[test]
    fn the_setting_and_windows_choose_the_palette() {
        let light = System::default();
        let dark = System {
            dark: true,
            high_contrast: false,
        };
        assert_eq!(resolve(ThemePreference::System, light), LIGHT);
        assert_eq!(resolve(ThemePreference::System, dark), DARK);
        assert_eq!(resolve(ThemePreference::Light, dark), LIGHT);
        assert_eq!(resolve(ThemePreference::Dark, light), DARK);
        let contrast = System {
            dark: true,
            high_contrast: true,
        };
        for preference in [
            ThemePreference::System,
            ThemePreference::Light,
            ThemePreference::Dark,
        ] {
            let palette = resolve(preference, contrast);
            assert!(
                palette.high_contrast && !palette.dark_frame,
                "{preference:?}"
            );
            assert_eq!(palette.selection_foreground, Some(palette.on_accent));
        }
    }

    #[test]
    fn a_change_is_reported_only_when_the_palette_changes() {
        set_system_for_test(System::default());
        set_preference(ThemePreference::System);
        assert_eq!(current(), LIGHT);
        assert!(
            !set_preference(ThemePreference::Light),
            "System is light already"
        );
        assert!(set_preference(ThemePreference::Dark));
        assert_eq!(current(), DARK);
        set_system_for_test(System {
            dark: true,
            high_contrast: false,
        });
        assert!(
            !set_preference(ThemePreference::System),
            "Windows is dark too"
        );
        set_system_for_test(System::default());
        assert_eq!(current(), LIGHT);
        set_preference(ThemePreference::System);
        assert_ne!(changed_message(), 0);
        assert_eq!(changed_message(), changed_message());
    }

    #[test]
    fn a_change_reaches_every_window_of_the_thread_shown_or_not() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, MSG, PM_REMOVE, PeekMessageW, WS_POPUP,
        };
        let window = unsafe {
            CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!window.is_null());
        tell_windows();
        let mut message: MSG = unsafe { std::mem::zeroed() };
        let got = unsafe {
            PeekMessageW(
                &mut message,
                window,
                changed_message(),
                changed_message(),
                PM_REMOVE,
            )
        };
        unsafe { DestroyWindow(window) };
        assert_ne!(got, 0, "the hidden window was told");
    }
}
