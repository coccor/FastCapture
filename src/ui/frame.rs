//! The frame FastCapture's windows share, FastPad's: the native caption is given up (the client
//! area is the whole window), and a painted title strip takes its place, with the title and
//! caption buttons drawn in the theme, the close button turning red. Windows keeps everything
//! else: dragging the strip, Snap (the maximize button reports `HTMAXBUTTON`, so Windows 11 shows
//! its layouts), Alt+Space, double-clicking to maximize, resizing from the edges, and the rounded
//! corners and shadow, which DWM draws for a captioned window.

// Window handles are only passed on to Windows, which checks them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::paint::{self, Align, GLYPH_CLOSE, GLYPH_MAXIMIZE, GLYPH_MINIMIZE, GLYPH_RESTORE, rect};
use super::theme::Palette;
use super::theme::metrics::{CAPTION_BUTTON, CAPTION_GLYPH, TITLE_STRIP, scale};
use super::theme::ramp::{self, TextStyle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DwmDefWindowProc, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetMonitorInfoW, HDC, HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromWindow, ScreenToClient,
};
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT,
    HTMAXBUTTON, HTMINBUTTON, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed, MINMAXINFO,
    NCCALCSIZE_PARAMS, PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SM_CXFRAME,
    SM_CXPADDEDBORDER, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowPos, SetWindowTextW, WA_INACTIVE, WM_ACTIVATE, WM_GETMINMAXINFO, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE,
    WM_NCMOUSEMOVE, WM_SIZE, WM_SYSCOMMAND,
};

/// The title's inset from the strip's left edge.
const TITLE_INSET: i32 = 16;

/// Which caption buttons a window has besides Close.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Buttons {
    pub minimize: bool,
    pub maximize: bool,
}

/// What a point of the window is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Client,
    Caption,
    Minimize,
    Maximize,
    Close,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Target {
    /// The `WM_NCHITTEST` answer.
    pub const fn code(self) -> u32 {
        match self {
            Self::Client => HTCLIENT,
            Self::Caption => HTCAPTION,
            Self::Minimize => HTMINBUTTON,
            Self::Maximize => HTMAXBUTTON,
            Self::Close => HTCLOSE,
            Self::Left => HTLEFT,
            Self::Right => HTRIGHT,
            Self::Top => HTTOP,
            Self::Bottom => HTBOTTOM,
            Self::TopLeft => HTTOPLEFT,
            Self::TopRight => HTTOPRIGHT,
            Self::BottomLeft => HTBOTTOMLEFT,
            Self::BottomRight => HTBOTTOMRIGHT,
        }
    }

    /// The caption button a non-client hit code names.
    pub fn button(code: usize) -> Option<Self> {
        match u32::try_from(code).ok()? {
            HTMINBUTTON => Some(Self::Minimize),
            HTMAXBUTTON => Some(Self::Maximize),
            HTCLOSE => Some(Self::Close),
            _ => None,
        }
    }
}

/// Where the strip's parts are in a window `width` wide.
#[derive(Clone, Copy)]
pub struct Layout {
    pub height: i32,
    pub title: RECT,
    pub minimize: Option<RECT>,
    pub maximize: Option<RECT>,
    pub close: RECT,
}

impl Layout {
    pub fn new(width: i32, dpi: u32, buttons: Buttons) -> Self {
        let height = scale(TITLE_STRIP, dpi);
        let button = scale(CAPTION_BUTTON, dpi).min(width / 4).max(1);
        let close = rect(width - button, 0, width, height);
        let mut left = close.left;
        let mut next = |on: bool| {
            on.then(|| {
                left -= button;
                rect(left, 0, left + button, height)
            })
        };
        let maximize = next(buttons.maximize);
        let minimize = next(buttons.minimize);
        Self {
            height,
            title: rect(scale(TITLE_INSET, dpi), 0, left, height),
            minimize,
            maximize,
            close,
        }
    }

    /// What (`x`, `y`) is in a window `width` × `height`. `border` is how wide the resizing
    /// edges are, or `None` for a window that can't be resized now (fixed, or maximized).
    pub fn hit(&self, x: i32, y: i32, width: i32, height: i32, border: Option<i32>) -> Target {
        let inside =
            |area: RECT| x >= area.left && x < area.right && y >= area.top && y < area.bottom;
        let button = [
            (Target::Close, Some(self.close)),
            (Target::Maximize, self.maximize),
            (Target::Minimize, self.minimize),
        ]
        .into_iter()
        .find_map(|(target, area)| area.filter(|area| inside(*area)).map(|_| target));
        // A caption button keeps its target to the window's edge, so the close button is easy
        // to hit in the corner.
        if let Some(target) = button {
            return target;
        }
        if let Some(target) = border.and_then(|border| edge(x, y, width, height, border)) {
            return target;
        }
        if y >= 0 && y < self.height {
            Target::Caption
        } else {
            Target::Client
        }
    }
}

/// The resizing edge or corner (`x`, `y`) is on in a window `width` × `height` whose edges are
/// `border` wide, or `None` inside them.
pub fn edge(x: i32, y: i32, width: i32, height: i32, border: i32) -> Option<Target> {
    let (left, right) = (x < border, x >= width - border);
    let (top, bottom) = (y < border, y >= height - border);
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(Target::TopLeft),
        (_, true, true, _) => Some(Target::TopRight),
        (true, _, _, true) => Some(Target::BottomLeft),
        (_, true, _, true) => Some(Target::BottomRight),
        (true, ..) => Some(Target::Left),
        (_, true, ..) => Some(Target::Right),
        (_, _, true, _) => Some(Target::Top),
        (_, _, _, true) => Some(Target::Bottom),
        _ => None,
    }
}

/// The pointer over the caption buttons: the one hovered, the one pressed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Pointer {
    hovered: Option<Target>,
    pressed: Option<Target>,
}

/// One window's frame. The window passes its messages to `handle` first, and paints the strip
/// with `paint` at the top of its client area.
pub struct Frame {
    title: String,
    buttons: Buttons,
    resizable: bool,
    pointer: Pointer,
    active: bool,
    dpi: u32,
    text_font: HFONT,
    glyph_font: HFONT,
}

impl Frame {
    pub fn new(title: &str, buttons: Buttons, resizable: bool, dpi: u32) -> Self {
        Self {
            title: title.to_owned(),
            buttons,
            resizable,
            pointer: Pointer::default(),
            active: true,
            dpi,
            text_font: ramp::font(TextStyle::Body, dpi),
            glyph_font: ramp::icon_font(CAPTION_GLYPH, dpi),
        }
    }

    /// The strip's height.
    pub fn height(&self) -> i32 {
        scale(TITLE_STRIP, self.dpi)
    }

    pub fn layout(&self, width: i32) -> Layout {
        Layout::new(width, self.dpi, self.buttons)
    }

    /// Makes the fonts for a new DPI or text size.
    pub fn set_dpi(&mut self, dpi: u32) {
        self.delete_fonts();
        self.dpi = dpi;
        self.text_font = ramp::font(TextStyle::Body, dpi);
        self.glyph_font = ramp::icon_font(CAPTION_GLYPH, dpi);
    }

    /// Shows `title` in the strip, and gives it to Windows for the taskbar and Alt+Tab.
    pub fn set_title(&mut self, window: HWND, title: &str) {
        self.title = title.to_owned();
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { SetWindowTextW(window, wide.as_ptr()) };
        self.invalidate(window);
    }

    fn invalidate(&self, window: HWND) {
        let mut client = rect(0, 0, 0, 0);
        unsafe { GetClientRect(window, &mut client) };
        let strip = rect(0, 0, client.right, self.height());
        unsafe { InvalidateRect(window, &strip, 0) };
    }

    fn set_pointer(&mut self, window: HWND, pointer: Pointer) {
        if pointer != self.pointer {
            self.pointer = pointer;
            self.invalidate(window);
        }
    }

    /// Handles the frame's own messages. `Some` is the answer when the frame took the message;
    /// `None` leaves it to the window (and `DefWindowProcW`).
    pub fn handle(
        &mut self,
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match message {
            WM_NCCALCSIZE => {
                // The client area is the whole window; a maximized window's is the monitor's
                // work area, since Windows makes it larger than the screen by its frame.
                if wparam != 0 && lparam != 0 && unsafe { IsZoomed(window) } != 0 {
                    let client = unsafe { &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0] };
                    if let Some(work) = work_area(window) {
                        *client = work;
                    }
                }
                Some(0)
            }
            WM_NCHITTEST => {
                let mut answer = 0;
                if unsafe { DwmDefWindowProc(window, message, wparam, lparam, &mut answer) } != 0 {
                    return Some(answer);
                }
                let mut point = POINT {
                    x: (lparam & 0xFFFF) as u16 as i16 as i32,
                    y: ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32,
                };
                unsafe { ScreenToClient(window, &mut point) };
                let mut client = rect(0, 0, 0, 0);
                unsafe { GetClientRect(window, &mut client) };
                let maximized = unsafe { IsZoomed(window) } != 0;
                let border = (self.resizable && !maximized).then(|| resize_border(self.dpi));
                let target = self.layout(client.right).hit(
                    point.x,
                    point.y,
                    client.right,
                    client.bottom,
                    border,
                );
                Some(target.code() as LRESULT)
            }
            WM_GETMINMAXINFO if self.buttons.maximize => {
                if let Some(work) = work_area(window) {
                    let monitor = monitor_rect(window).unwrap_or(work);
                    let limits = unsafe { &mut *(lparam as *mut MINMAXINFO) };
                    limits.ptMaxPosition = POINT {
                        x: work.left - monitor.left,
                        y: work.top - monitor.top,
                    };
                    limits.ptMaxSize = POINT {
                        x: work.right - work.left,
                        y: work.bottom - work.top,
                    };
                }
                None
            }
            WM_NCMOUSEMOVE => {
                let target = Target::button(wparam);
                if target.is_some() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut track) };
                }
                self.set_pointer(
                    window,
                    Pointer {
                        hovered: target,
                        ..self.pointer
                    },
                );
                None
            }
            WM_NCMOUSELEAVE => {
                self.set_pointer(window, Pointer::default());
                None
            }
            // `DefWindowProcW` would track the press with its own classic buttons.
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
                let target = Target::button(wparam)?;
                self.set_pointer(
                    window,
                    Pointer {
                        hovered: Some(target),
                        pressed: Some(target),
                    },
                );
                Some(0)
            }
            WM_NCLBUTTONUP => {
                let target = Target::button(wparam);
                let pressed = self.pointer.pressed;
                self.set_pointer(
                    window,
                    Pointer {
                        pressed: None,
                        ..self.pointer
                    },
                );
                let target = target?;
                if pressed == Some(target) {
                    let command = match target {
                        Target::Minimize => SC_MINIMIZE,
                        Target::Maximize if unsafe { IsZoomed(window) } != 0 => SC_RESTORE,
                        Target::Maximize => SC_MAXIMIZE,
                        _ => SC_CLOSE,
                    };
                    // Posted: the window may still be inside this message's handling.
                    unsafe { PostMessageW(window, WM_SYSCOMMAND, command as WPARAM, 0) };
                }
                Some(0)
            }
            WM_ACTIVATE => {
                self.active = (wparam & 0xFFFF) as u32 != WA_INACTIVE;
                self.invalidate(window);
                None
            }
            WM_SIZE => {
                self.invalidate(window);
                None
            }
            _ => None,
        }
    }

    /// Paints the strip across the top of `dc`, a window `width` wide.
    pub fn paint(&self, dc: HDC, width: i32, palette: &Palette, maximized: bool) {
        let layout = self.layout(width);
        paint::fill(
            dc,
            rect(0, 0, width, layout.height),
            palette.strip_background,
        );
        let title = if self.active {
            palette.strip_foreground
        } else {
            palette.muted_foreground
        };
        paint::text(
            dc,
            &self.title,
            layout.title,
            self.text_font,
            title,
            Align::Left,
        );
        let maximize_glyph = if maximized {
            GLYPH_RESTORE
        } else {
            GLYPH_MAXIMIZE
        };
        for (target, area, glyph) in [
            (Target::Minimize, layout.minimize, GLYPH_MINIMIZE),
            (Target::Maximize, layout.maximize, maximize_glyph),
            (Target::Close, Some(layout.close), GLYPH_CLOSE),
        ] {
            let Some(area) = area else { continue };
            let hovered = self.pointer.hovered == Some(target);
            let pressed = hovered && self.pointer.pressed == Some(target);
            let (background, foreground) = match (target, hovered, pressed) {
                (Target::Close, true, true) => (
                    Some(palette.close_pressed_background),
                    palette.close_hover_foreground,
                ),
                (Target::Close, true, false) => (
                    Some(palette.close_hover_background),
                    palette.close_hover_foreground,
                ),
                (_, true, true) => (Some(palette.pressed_background), palette.hover_foreground),
                (_, true, false) => (Some(palette.hover_background), palette.hover_foreground),
                _ if !self.active => (None, palette.muted_foreground),
                _ => (None, palette.strip_foreground),
            };
            if let Some(background) = background {
                paint::fill(dc, area, background);
            }
            paint::text(dc, glyph, area, self.glyph_font, foreground, Align::Center);
        }
    }

    fn delete_fonts(&mut self) {
        unsafe {
            DeleteObject(self.text_font as HGDIOBJ);
            DeleteObject(self.glyph_font as HGDIOBJ);
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        self.delete_fonts();
    }
}

/// Gives up `window`'s native caption and applies `palette` to its frame. Call it once the
/// window exists, outside the window's own message handling: Windows asks the frame for the new
/// client area at once.
pub fn attach(window: HWND, palette: &Palette) {
    apply_theme(window, palette);
    unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// How wide a resizable window's resizing edges are, as Windows' own frame would be.
pub fn resize_border(dpi: u32) -> i32 {
    unsafe {
        GetSystemMetricsForDpi(SM_CXFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
    .max(scale(4, dpi))
}

fn monitor_info(window: HWND) -> Option<MONITORINFO> {
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info)
}

fn work_area(window: HWND) -> Option<RECT> {
    monitor_info(window).map(|info| info.rcWork)
}

fn monitor_rect(window: HWND) -> Option<RECT> {
    monitor_info(window).map(|info| info.rcMonitor)
}

/// The frame's look in `palette`: Windows' dark frame when the palette is dark, and rounded
/// corners except in high contrast. Failures keep Windows' defaults.
pub fn apply_theme(window: HWND, palette: &Palette) {
    let dark = i32::from(palette.dark_frame);
    let corners = if palette.high_contrast {
        DWMWCP_DONOTROUND
    } else {
        DWMWCP_ROUND
    };
    unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        );
        DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const corners).cast(),
            size_of_val(&corners) as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::paint::tests::Canvas;
    use crate::ui::theme::{DARK, LIGHT};
    use std::cell::RefCell;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, IsWindow, RegisterClassW,
        SendMessageW, WNDCLASSW, WS_CAPTION, WS_OVERLAPPEDWINDOW, WS_SYSMENU,
    };

    const ALL: Buttons = Buttons {
        minimize: true,
        maximize: true,
    };

    #[test]
    fn the_strip_scales_and_the_buttons_sit_in_the_top_right_corner() {
        for dpi in [96, 144, 192] {
            let layout = Layout::new(800, dpi, ALL);
            let (button, height) = (scale(46, dpi), scale(40, dpi));
            assert_eq!(layout.height, height);
            let close = layout.close;
            assert_eq!(
                (close.left, close.top, close.right, close.bottom),
                (800 - button, 0, 800, height),
                "{dpi}"
            );
            assert_eq!(layout.maximize.unwrap().right, layout.close.left);
            assert_eq!(
                layout.minimize.unwrap().right,
                layout.maximize.unwrap().left
            );
            assert_eq!(layout.title.right, layout.minimize.unwrap().left);
            assert_eq!(layout.title.left, scale(16, dpi));
        }
        let dialog = Layout::new(400, 96, Buttons::default());
        assert!(dialog.minimize.is_none() && dialog.maximize.is_none());
        assert_eq!(dialog.title.right, dialog.close.left);
    }

    #[test]
    fn edges_resize_and_the_middle_does_not() {
        assert_eq!(edge(0, 0, 500, 300, 6), Some(Target::TopLeft));
        assert_eq!(edge(499, 150, 500, 300, 6), Some(Target::Right));
        assert_eq!(edge(250, 299, 500, 300, 6), Some(Target::Bottom));
        assert_eq!(edge(250, 150, 500, 300, 6), None);
    }

    #[test]
    fn edges_resize_the_strip_drags_and_caption_buttons_win_in_their_corner() {
        let layout = Layout::new(800, 96, ALL);
        let hit = |x, y, border| layout.hit(x, y, 800, 600, border);
        let border = Some(8);
        assert_eq!(
            hit(799, 0, border),
            Target::Close,
            "the corner is the close button's"
        );
        assert_eq!(hit(730, 20, border), Target::Maximize);
        assert_eq!(hit(680, 20, border), Target::Minimize);
        assert_eq!(hit(0, 0, border), Target::TopLeft);
        assert_eq!(hit(300, 2, border), Target::Top);
        assert_eq!(hit(300, 20, border), Target::Caption);
        assert_eq!(hit(2, 300, border), Target::Left);
        assert_eq!(hit(797, 300, border), Target::Right);
        assert_eq!(hit(300, 598, border), Target::Bottom);
        assert_eq!(hit(799, 599, border), Target::BottomRight);
        assert_eq!(hit(0, 599, border), Target::BottomLeft);
        assert_eq!(hit(300, 300, border), Target::Client);
        assert_eq!(hit(300, 2, None), Target::Caption, "maximized or fixed");
        assert_eq!(hit(2, 300, None), Target::Client);
        assert_eq!(Target::button(HTCLOSE as usize), Some(Target::Close));
        assert_eq!(Target::button(HTCAPTION as usize), None);
    }

    #[test]
    fn the_strip_paints_the_title_and_a_red_close_when_hovered() {
        let mut frame = Frame::new("Settings", ALL, true, 96);
        let canvas = Canvas::new(400, 40);
        frame.paint(canvas.dc, 400, &DARK, false);
        assert_eq!(canvas.pixel(200, 39), DARK.strip_background);
        assert_eq!(canvas.pixel(399, 0), DARK.strip_background, "resting close");
        let inked = (16..120)
            .flat_map(|x| (8..32).map(move |y| (x, y)))
            .any(|(x, y)| canvas.pixel(x, y) != DARK.strip_background);
        assert!(inked, "the title is drawn");
        frame.pointer.hovered = Some(Target::Close);
        frame.paint(canvas.dc, 400, &LIGHT, false);
        assert_eq!(canvas.pixel(399, 0), LIGHT.close_hover_background);
        frame.pointer.pressed = Some(Target::Close);
        frame.paint(canvas.dc, 400, &LIGHT, false);
        assert_eq!(canvas.pixel(399, 0), LIGHT.close_pressed_background);
    }

    thread_local! {
        static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
    }

    extern "system" fn test_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let handled = FRAME.with(|frame| {
            frame
                .borrow_mut()
                .as_mut()
                .and_then(|frame| frame.handle(window, message, wparam, lparam))
        });
        handled.unwrap_or_else(|| unsafe { DefWindowProcW(window, message, wparam, lparam) })
    }

    fn framed_window(style: u32) -> HWND {
        let class: Vec<u16> = "FastCaptureFrameTest\0".encode_utf16().collect();
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let description = WNDCLASSW {
            lpfnWndProc: Some(test_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassW(&description) };
        unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                style,
                100,
                100,
                500,
                300,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        }
    }

    /// Handles the thread's waiting messages.
    fn pump() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        };
        let mut message: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe { DispatchMessageW(&message) };
        }
    }

    fn hit_at(window: HWND, x: i32, y: i32) -> u32 {
        let mut point = POINT { x, y };
        unsafe { ClientToScreen(window, &mut point) };
        let lparam = ((point.y as u16 as u32) << 16 | point.x as u16 as u32) as LPARAM;
        unsafe { SendMessageW(window, WM_NCHITTEST, 0, lparam) as u32 }
    }

    #[test]
    fn a_framed_window_is_all_client_hit_tests_its_strip_and_closes_from_its_button() {
        FRAME.with(|frame| *frame.borrow_mut() = Some(Frame::new("Test", ALL, true, 96)));
        let window = framed_window(WS_OVERLAPPEDWINDOW);
        assert!(!window.is_null());
        attach(window, &DARK);
        let mut outer = rect(0, 0, 0, 0);
        let mut client = rect(0, 0, 0, 0);
        unsafe {
            GetWindowRect(window, &mut outer);
            GetClientRect(window, &mut client);
        }
        assert_eq!(
            (client.right, client.bottom),
            (outer.right - outer.left, outer.bottom - outer.top),
            "no native caption or border"
        );
        let width = client.right;
        assert_eq!(hit_at(window, width / 2, 20), HTCAPTION);
        assert_eq!(hit_at(window, width - 5, 20), HTCLOSE);
        assert_eq!(hit_at(window, width / 2, 150), HTCLIENT);
        assert_eq!(hit_at(window, 1, 150), HTLEFT);

        unsafe {
            SendMessageW(window, WM_NCLBUTTONDOWN, HTCLOSE as WPARAM, 0);
            SendMessageW(window, WM_NCLBUTTONUP, HTCLOSE as WPARAM, 0);
        }
        pump();
        assert_eq!(unsafe { IsWindow(window) }, 0, "Close closed it");
        FRAME.with(|frame| *frame.borrow_mut() = None);

        FRAME.with(|frame| {
            *frame.borrow_mut() = Some(Frame::new("Fixed", Buttons::default(), false, 96))
        });
        let fixed = framed_window(WS_CAPTION | WS_SYSMENU);
        attach(fixed, &LIGHT);
        assert_eq!(
            hit_at(fixed, 1, 150),
            HTCLIENT,
            "a fixed window doesn't resize"
        );
        assert_eq!(hit_at(fixed, 100, 2), HTCAPTION);
        unsafe { DestroyWindow(fixed) };
        FRAME.with(|frame| *frame.borrow_mut() = None);
    }
}
