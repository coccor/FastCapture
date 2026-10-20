//! FastCapture's standard controls, painted in the theme. Each control stays the Windows control
//! it is: it keeps its own keyboard handling, focus, state and what screen readers say about it;
//! FastCapture only paints it, subclassed, from `theme::current()` each time, so a theme change is
//! only a repaint. Push buttons get rounded faces (the default button in the accent), check boxes
//! become FastPad's toggle switches, radio buttons a segmented choice, combo boxes and fields a
//! rounded border with the focus ring; list views and list boxes get FastPad's rows.

// Window handles and device contexts are only passed on to Windows, which checks them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::paint::{self, Align, Corners, GLYPH_CHEVRON_DOWN, blend, rect};
use super::theme::metrics::{
    CONTROL_RADIUS, FOCUS_GAP, FOCUS_RING, ROW_INSET_X, ROW_INSET_Y, SELECTION_BAR_HEIGHT,
    SELECTION_BAR_WIDTH, TOGGLE_HEIGHT, TOGGLE_WIDTH, scale,
};
use super::theme::{self, Palette, ramp};
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateSolidBrush, ExcludeClipRect, GetWindowDC, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect,
    RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC, SetBkColor,
    SetTextColor,
};
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_SKIPDEFAULT, COMBOBOXINFO, DRAWITEMSTRUCT, EM_SETMARGINS, GetComboBoxInfo, HDI_TEXT,
    HDITEMW, HDM_GETITEMCOUNT, HDM_GETITEMRECT, HDM_GETITEMW, LVIR_BOUNDS, LVIR_LABEL,
    LVIS_SELECTED, LVITEMW, LVM_GETHEADER, LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETITEMTEXTW,
    LVM_GETSUBITEMRECT, LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR, NM_CUSTOMDRAW, NMHDR,
    NMLVCUSTOMDRAW, ODS_FOCUS, ODS_SELECTED, SetWindowTheme, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, IsWindowEnabled, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_GETSTATE, BM_SETCHECK, BM_SETSTATE, BS_DEFPUSHBUTTON, BS_TYPEMASK, BST_PUSHED,
    CB_GETCURSEL, CB_GETDROPPEDSTATE, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_SETCURSEL,
    CBS_DROPDOWNLIST, EC_LEFTMARGIN, EC_RIGHTMARGIN, EN_KILLFOCUS, EN_SETFOCUS, ES_READONLY,
    EnumChildWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetClientRect, GetWindowLongW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsChild, LB_GETTEXT, LB_GETTEXTLEN,
    SendMessageW, SetWindowLongW, UISF_HIDEFOCUS, WM_CAPTURECHANGED, WM_COMMAND, WM_CTLCOLORBTN,
    WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_ENABLE, WM_ERASEBKGND, WM_GETFONT,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY,
    WM_NCPAINT, WM_PAINT, WM_PRINTCLIENT, WM_QUERYUISTATE, WM_SETFOCUS, WM_SETTEXT,
    WM_UPDATEUISTATE, WS_BORDER, WS_CLIPCHILDREN, WS_EX_CLIENTEDGE, WS_HSCROLL, WS_VSCROLL,
};

/// What a control sits on, so its corners blend into it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Surface {
    Strip,
    Content,
}

impl Surface {
    pub fn color(self, palette: &Palette) -> u32 {
        match self {
            Self::Strip => palette.strip_background,
            Self::Content => palette.content_background,
        }
    }
}

/// Where a radio button sits in its segmented choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Segment {
    First,
    Middle,
    Last,
    Only,
}

/// How a control is painted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Look {
    /// A push button; the default button (`BS_DEFPUSHBUTTON`) is filled with the accent.
    Button,
    /// A check box, drawn as a toggle switch with its text beside it.
    Toggle,
    /// A radio button, drawn as one segment of a choice.
    Segment(Segment),
    /// A combo box: a field with a chevron, its list and edit in the theme.
    Combo,
    /// An edit or a list box: its border rounded, the accent when it has the focus.
    Field,
    /// A list view's header.
    Header,
}

struct State {
    look: Look,
    behind: Surface,
    hot: bool,
}

const SUBCLASS: usize = 0x4643_4C4B;

/// Paints `control` as `look` on `behind` from now on, until it is destroyed.
pub fn paint_as(control: HWND, look: Look, behind: Surface) {
    let palette = theme::current();
    let dpi = unsafe { GetDpiForWindow(control) };
    match look {
        Look::Field if class_of(control).eq_ignore_ascii_case("Edit") => {
            let margin = scale(6, dpi) as usize;
            let margins = margin | (margin << 16);
            unsafe {
                SendMessageW(
                    control,
                    EM_SETMARGINS,
                    (EC_LEFTMARGIN | EC_RIGHTMARGIN) as WPARAM,
                    margins as LPARAM,
                )
            };
        }
        Look::Combo => unsafe {
            // Its edit is a child: painting the face mustn't cover it.
            let style = GetWindowLongW(control, GWL_STYLE) as u32;
            SetWindowLongW(control, GWL_STYLE, (style | WS_CLIPCHILDREN) as i32);
        },
        _ => {}
    }
    let state = Box::new(State {
        look,
        behind,
        hot: false,
    });
    unsafe {
        SetWindowSubclass(
            control,
            Some(subclass_proc),
            SUBCLASS,
            Box::into_raw(state) as usize,
        )
    };
    theme_scrollbars(control, &palette);
    unsafe { InvalidateRect(control, std::ptr::null(), 1) };
}

fn class_of(window: HWND) -> String {
    let mut name = [0u16; 64];
    let length = unsafe { GetClassNameW(window, name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..length.max(0) as usize])
}

/// Windows' dark or light scroll bars for a control that has them, and for a combo box's list.
pub fn theme_scrollbars(control: HWND, palette: &Palette) {
    let class = class_of(control);
    let style = unsafe { GetWindowLongW(control, GWL_STYLE) } as u32;
    let theme: Vec<u16> = if palette.dark_frame {
        "DarkMode_Explorer\0"
    } else {
        "Explorer\0"
    }
    .encode_utf16()
    .collect();
    let scrolls = style & (WS_VSCROLL | WS_HSCROLL) != 0
        || class.eq_ignore_ascii_case("SysListView32")
        || class.eq_ignore_ascii_case("ListBox");
    if class.eq_ignore_ascii_case("ComboBox") {
        let mut info: COMBOBOXINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<COMBOBOXINFO>() as u32;
        if unsafe { GetComboBoxInfo(control, &mut info) } != 0 && !info.hwndList.is_null() {
            unsafe { SetWindowTheme(info.hwndList, theme.as_ptr(), std::ptr::null()) };
        }
    } else if scrolls {
        unsafe { SetWindowTheme(control, theme.as_ptr(), std::ptr::null()) };
    }
}

/// After a theme change: every child of `window` gets the new scroll bars and list colors, and
/// everything is painted again.
pub fn retheme(window: HWND) {
    unsafe extern "system" fn each(child: HWND, _: LPARAM) -> i32 {
        let palette = theme::current();
        theme_scrollbars(child, &palette);
        if class_of(child).eq_ignore_ascii_case("SysListView32") {
            list_colors(child, &palette);
        }
        1
    }
    unsafe {
        EnumChildWindows(window, Some(each), 0);
        RedrawWindow(
            window,
            std::ptr::null(),
            std::ptr::null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
        );
    }
}

/// A solid brush of `color`, kept for the life of the thread: there are only a few.
pub fn brush(color: u32) -> HBRUSH {
    thread_local! {
        static BRUSHES: RefCell<Vec<(u32, HBRUSH)>> = const { RefCell::new(Vec::new()) };
    }
    BRUSHES.with(|brushes| {
        let mut brushes = brushes.borrow_mut();
        if let Some((_, brush)) = brushes.iter().find(|(known, _)| *known == color) {
            return *brush;
        }
        let brush = unsafe { CreateSolidBrush(color) };
        brushes.push((color, brush));
        brush
    })
}

/// The window's answer to `WM_CTLCOLOR*` for its children, which sit on `behind`: fields and
/// lists in the content colors, labels and buttons on the window's own. `None` for other
/// messages.
pub fn colors(message: u32, wparam: WPARAM, lparam: LPARAM, behind: Surface) -> Option<LRESULT> {
    let palette = theme::current();
    let dc = wparam as HDC;
    let child = lparam as HWND;
    let enabled = unsafe { IsWindowEnabled(child) } != 0;
    let (text, background) = match message {
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            (palette.content_foreground, palette.content_background)
        }
        // Read-only and disabled edits ask as statics do.
        WM_CTLCOLORSTATIC if class_of(child).eq_ignore_ascii_case("Edit") => {
            (palette.content_foreground, palette.content_background)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => (palette.content_foreground, behind.color(&palette)),
        _ => return None,
    };
    let text = if enabled {
        text
    } else {
        palette.disabled_foreground
    };
    unsafe {
        SetTextColor(dc, text);
        SetBkColor(dc, background);
    }
    Some(brush(background) as LRESULT)
}

unsafe extern "system" fn subclass_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let state = unsafe { &mut *(data as *mut State) };
    let painted = !matches!(state.look, Look::Field);
    match message {
        WM_NCDESTROY => unsafe {
            RemoveWindowSubclass(window, Some(subclass_proc), SUBCLASS);
            drop(Box::from_raw(data as *mut State));
            return DefSubclassProc(window, message, wparam, lparam);
        },
        WM_ERASEBKGND if painted => return 1,
        WM_PAINT if painted => {
            paint::buffered(window, |dc, area| draw(window, dc, area, state));
            return 0;
        }
        WM_PRINTCLIENT if painted => {
            let mut client = rect(0, 0, 0, 0);
            unsafe { GetClientRect(window, &mut client) };
            paint::in_memory(wparam as HDC, client, |dc, area| {
                draw(window, dc, area, state)
            });
            return 0;
        }
        WM_MOUSEMOVE if !state.hot => {
            state.hot = true;
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: window,
                dwHoverTime: 0,
            };
            unsafe {
                TrackMouseEvent(&mut track);
                InvalidateRect(window, std::ptr::null(), 0);
            }
        }
        WM_MOUSELEAVE => {
            state.hot = false;
            unsafe { InvalidateRect(window, std::ptr::null(), 0) };
        }
        WM_NCPAINT if !painted => {
            let answer = unsafe { DefSubclassProc(window, message, wparam, lparam) };
            draw_field_border(window, state);
            return answer;
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX if state.look == Look::Combo => {
            return colors(message, wparam, lparam, state.behind).unwrap_or(0);
        }
        _ => {}
    }
    let answer = unsafe { DefSubclassProc(window, message, wparam, lparam) };
    let field_focus = matches!(message, WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE);
    let combo_edit_focus = state.look == Look::Combo
        && message == WM_COMMAND
        && matches!(((wparam >> 16) & 0xFFFF) as u32, EN_SETFOCUS | EN_KILLFOCUS);
    if !painted && field_focus {
        unsafe {
            RedrawWindow(
                window,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_FRAME | RDW_INVALIDATE,
            )
        };
    }
    // A button draws itself at once on a click, a key or a change of state: paint over it.
    let changed = matches!(
        message,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_KEYDOWN
            | WM_KEYUP
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_ENABLE
            | WM_SETTEXT
            | WM_CAPTURECHANGED
            | WM_UPDATEUISTATE
            | BM_SETSTATE
            | BM_SETCHECK
            | CB_SETCURSEL
    );
    if painted && (changed || combo_edit_focus) {
        unsafe { InvalidateRect(window, std::ptr::null(), 0) };
    }
    answer
}

/// Whether the focus should be shown: Windows hides it until the keyboard is used.
fn shows_focus(window: HWND) -> bool {
    unsafe {
        GetFocus() == window
            && SendMessageW(window, WM_QUERYUISTATE, 0, 0) as u32 & UISF_HIDEFOCUS == 0
    }
}

fn font_of(window: HWND) -> HFONT {
    unsafe { SendMessageW(window, WM_GETFONT, 0, 0) as HFONT }
}

fn text_of(window: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(window) };
    let mut text = vec![0u16; length.max(0) as usize + 1];
    let read = unsafe { GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) };
    String::from_utf16_lossy(&text[..read.max(0) as usize])
}

/// A fill a little lighter or darker under the pointer and when pressed, toward `ink`.
fn tone(base: u32, ink: u32, hot: bool, pressed: bool, palette: &Palette) -> u32 {
    if palette.high_contrast {
        return if hot || pressed {
            palette.hover_background
        } else {
            base
        };
    }
    match (hot, pressed) {
        (_, true) => blend(ink, base, 40),
        (true, false) => blend(ink, base, 20),
        _ => base,
    }
}

fn draw(window: HWND, dc: HDC, area: RECT, state: &State) {
    let palette = theme::current();
    let dpi = unsafe { GetDpiForWindow(window) };
    let behind = state.behind.color(&palette);
    paint::fill(dc, area, behind);
    match state.look {
        Look::Button => draw_button(window, dc, area, state, &palette, dpi),
        Look::Toggle => draw_toggle(window, dc, area, state, &palette, dpi),
        Look::Segment(segment) => draw_segment(window, dc, area, state, segment, &palette, dpi),
        Look::Combo => draw_combo(window, dc, area, state, &palette, dpi),
        Look::Header => draw_header(window, dc, area, &palette, dpi),
        Look::Field => {}
    }
}

fn draw_button(window: HWND, dc: HDC, area: RECT, state: &State, palette: &Palette, dpi: u32) {
    let style = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32;
    let enabled = unsafe { IsWindowEnabled(window) } != 0;
    let pressed = unsafe { SendMessageW(window, BM_GETSTATE, 0, 0) } as u32 & BST_PUSHED != 0;
    let primary = enabled && style & BS_TYPEMASK as u32 == BS_DEFPUSHBUTTON as u32;
    let behind = state.behind.color(palette);
    let radius = paint::radius(palette, CONTROL_RADIUS, dpi);
    let text = if !enabled {
        palette.disabled_foreground
    } else if primary {
        palette.on_accent
    } else if palette.high_contrast && (state.hot || pressed) {
        palette.hover_foreground
    } else {
        palette.content_foreground
    };
    if primary {
        let face = tone(
            palette.accent,
            palette.on_accent,
            state.hot,
            pressed,
            palette,
        );
        paint::fill_rounded(dc, area, radius, Corners::ALL, face, behind);
    } else {
        let face = tone(
            palette.hover_background,
            palette.content_foreground,
            state.hot && enabled,
            pressed,
            palette,
        );
        paint::fill_bordered(dc, area, radius, face, palette.stroke, behind);
    }
    paint::text(
        dc,
        &text_of(window),
        area,
        font_of(window),
        text,
        Align::Center,
    );
    if shows_focus(window) {
        paint::focus_ring(dc, area, palette, dpi);
    }
}

/// Where a toggle's track is in a control `area`, leaving room for the focus ring around it.
pub fn toggle_track(area: RECT, dpi: u32) -> RECT {
    let ring = scale(FOCUS_RING + FOCUS_GAP, dpi);
    let height = scale(TOGGLE_HEIGHT, dpi);
    let top = area.top + (area.bottom - area.top - height) / 2;
    rect(
        area.left + ring,
        top,
        area.left + ring + scale(TOGGLE_WIDTH, dpi),
        top + height,
    )
}

fn draw_toggle(window: HWND, dc: HDC, area: RECT, state: &State, palette: &Palette, dpi: u32) {
    let enabled = unsafe { IsWindowEnabled(window) } != 0;
    let on = unsafe { SendMessageW(window, BM_GETCHECK, 0, 0) } as u32 == BST_CHECKED;
    let behind = state.behind.color(palette);
    let track = toggle_track(area, dpi);
    let round = paint::radius(palette, TOGGLE_HEIGHT / 2, dpi);
    let (fill, knob) = if on {
        let fill = if enabled {
            tone(palette.accent, palette.on_accent, state.hot, false, palette)
        } else {
            palette.disabled_foreground
        };
        paint::fill_rounded(dc, track, round, Corners::ALL, fill, behind);
        (fill, palette.on_accent)
    } else {
        let ink = if enabled {
            palette.muted_foreground
        } else {
            palette.disabled_foreground
        };
        let fill = tone(
            palette.content_background,
            palette.content_foreground,
            state.hot && enabled,
            false,
            palette,
        );
        paint::fill_bordered(dc, track, round, fill, ink, behind);
        (fill, ink)
    };
    let size = scale(if state.hot && enabled { 14 } else { 12 }, dpi);
    let inset = (track.bottom - track.top - size) / 2;
    let left = if on {
        track.right - inset - size
    } else {
        track.left + inset
    };
    let knob_rect = rect(
        left,
        track.top + inset,
        left + size,
        track.top + inset + size,
    );
    paint::fill_rounded(
        dc,
        knob_rect,
        paint::radius(palette, 7, dpi).min(size / 2),
        Corners::ALL,
        knob,
        fill,
    );
    let text = if enabled {
        palette.content_foreground
    } else {
        palette.disabled_foreground
    };
    let label = rect(
        track.right + scale(8, dpi),
        area.top,
        area.right,
        area.bottom,
    );
    paint::text(
        dc,
        &text_of(window),
        label,
        font_of(window),
        text,
        Align::Left,
    );
    if shows_focus(window) {
        let ring = scale(FOCUS_RING + FOCUS_GAP, dpi);
        let around = rect(
            track.left - ring,
            track.top - ring,
            track.right + ring,
            track.bottom + ring,
        );
        paint::focus_ring(dc, around, palette, dpi);
    }
}

fn draw_segment(
    window: HWND,
    dc: HDC,
    area: RECT,
    state: &State,
    segment: Segment,
    palette: &Palette,
    dpi: u32,
) {
    let enabled = unsafe { IsWindowEnabled(window) } != 0;
    let on = unsafe { SendMessageW(window, BM_GETCHECK, 0, 0) } as u32 == BST_CHECKED;
    let behind = state.behind.color(palette);
    let radius = paint::radius(palette, CONTROL_RADIUS, dpi);
    let corners = match segment {
        Segment::First => Corners::LEFT,
        Segment::Middle => Corners::NONE,
        Segment::Last => Corners::RIGHT,
        Segment::Only => Corners::ALL,
    };
    let text = if !enabled {
        palette.disabled_foreground
    } else if on {
        palette.on_accent
    } else {
        palette.content_foreground
    };
    if on && enabled {
        let face = tone(palette.accent, palette.on_accent, state.hot, false, palette);
        paint::fill_rounded(dc, area, radius, corners, face, behind);
    } else {
        let face = tone(
            palette.hover_background,
            palette.content_foreground,
            state.hot && enabled,
            false,
            palette,
        );
        paint::fill_bordered_corners(dc, area, radius, corners, face, palette.stroke, behind);
    }
    paint::text(
        dc,
        &text_of(window),
        area,
        font_of(window),
        text,
        Align::Center,
    );
    if shows_focus(window) {
        paint::focus_ring(dc, area, palette, dpi);
    }
}

fn draw_combo(window: HWND, dc: HDC, area: RECT, state: &State, palette: &Palette, dpi: u32) {
    let style = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32;
    let enabled = unsafe { IsWindowEnabled(window) } != 0;
    let dropped = unsafe { SendMessageW(window, CB_GETDROPPEDSTATE, 0, 0) } != 0;
    let focus = unsafe { GetFocus() };
    let focused = focus == window || (!focus.is_null() && unsafe { IsChild(window, focus) } != 0);
    let behind = state.behind.color(palette);
    let radius = paint::radius(palette, CONTROL_RADIUS, dpi);
    let border = if (state.hot || dropped) && enabled {
        palette.muted_foreground
    } else {
        palette.stroke
    };
    paint::fill_bordered(dc, area, radius, palette.content_background, border, behind);
    let chevron = rect(
        area.right - scale(28, dpi),
        area.top,
        area.right - scale(4, dpi),
        area.bottom,
    );
    if style & 0x3 == CBS_DROPDOWNLIST as u32 {
        let index = unsafe { SendMessageW(window, CB_GETCURSEL, 0, 0) };
        let text = if index >= 0 {
            let length = unsafe { SendMessageW(window, CB_GETLBTEXTLEN, index as WPARAM, 0) };
            let mut buffer = vec![0u16; length.max(0) as usize + 1];
            unsafe {
                SendMessageW(
                    window,
                    CB_GETLBTEXT,
                    index as WPARAM,
                    buffer.as_mut_ptr() as LPARAM,
                )
            };
            String::from_utf16_lossy(&buffer[..length.max(0) as usize])
        } else {
            String::new()
        };
        let color = if enabled {
            palette.content_foreground
        } else {
            palette.disabled_foreground
        };
        let label = rect(
            area.left + scale(8, dpi),
            area.top,
            chevron.left,
            area.bottom,
        );
        paint::text(dc, &text, label, font_of(window), color, Align::Left);
    }
    let glyphs = ramp::icon_font(10, dpi);
    let ink = if enabled {
        palette.muted_foreground
    } else {
        palette.disabled_foreground
    };
    paint::text(dc, GLYPH_CHEVRON_DOWN, chevron, glyphs, ink, Align::Center);
    unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(glyphs as HGDIOBJ) };
    let keyboard =
        unsafe { SendMessageW(window, WM_QUERYUISTATE, 0, 0) } as u32 & UISF_HIDEFOCUS == 0;
    if focused && keyboard {
        paint::focus_ring(dc, area, palette, dpi);
    }
}

/// An edit's or a list box's border, over the one Windows drew: 1 px for `WS_BORDER`, 2 px for
/// `WS_EX_CLIENTEDGE`, rounded, the accent when the field has the focus.
fn draw_field_border(window: HWND, state: &State) {
    let style = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongW(window, GWL_EXSTYLE) } as u32;
    let width = if extended & WS_EX_CLIENTEDGE != 0 {
        2
    } else if style & WS_BORDER != 0 {
        1
    } else {
        return;
    };
    let palette = theme::current();
    let dpi = unsafe { GetDpiForWindow(window) };
    let mut outer = rect(0, 0, 0, 0);
    unsafe { GetWindowRect(window, &mut outer) };
    let area = rect(0, 0, outer.right - outer.left, outer.bottom - outer.top);
    let read_only =
        style & ES_READONLY as u32 != 0 && class_of(window).eq_ignore_ascii_case("Edit");
    let focused = unsafe { GetFocus() } == window && !read_only;
    let edge = if focused {
        palette.accent
    } else {
        palette.stroke
    };
    let inner = if focused && width == 2 {
        palette.accent
    } else {
        palette.content_background
    };
    let radius = paint::radius(&palette, CONTROL_RADIUS, dpi);
    let dc = unsafe { GetWindowDC(window) };
    if dc.is_null() {
        return;
    }
    unsafe {
        ExcludeClipRect(
            dc,
            area.left + width,
            area.top + width,
            area.right - width,
            area.bottom - width,
        )
    };
    let behind = state.behind.color(&palette);
    // Windows' own border is still under the corners the rounding leaves out.
    paint::fill(dc, area, behind);
    paint::fill_rounded(dc, area, radius, Corners::ALL, edge, behind);
    if width == 2 {
        let ring = rect(1, 1, area.right - 1, area.bottom - 1);
        paint::fill_rounded(dc, ring, (radius - 1).max(0), Corners::ALL, inner, edge);
    }
    unsafe { ReleaseDC(window, dc) };
}

fn draw_header(window: HWND, dc: HDC, area: RECT, palette: &Palette, dpi: u32) {
    paint::fill(dc, area, palette.content_background);
    let count = unsafe { SendMessageW(window, HDM_GETITEMCOUNT, 0, 0) }.max(0);
    let font = font_of(window);
    for index in 0..count {
        let mut item = rect(0, 0, 0, 0);
        unsafe {
            SendMessageW(
                window,
                HDM_GETITEMRECT,
                index as WPARAM,
                (&raw mut item) as LPARAM,
            )
        };
        let mut buffer = [0u16; 128];
        let mut info: HDITEMW = unsafe { std::mem::zeroed() };
        info.mask = HDI_TEXT;
        info.pszText = buffer.as_mut_ptr();
        info.cchTextMax = buffer.len() as i32;
        unsafe {
            SendMessageW(
                window,
                HDM_GETITEMW,
                index as WPARAM,
                (&raw mut info) as LPARAM,
            )
        };
        let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..length]);
        let label = rect(
            item.left + scale(10, dpi),
            item.top,
            item.right - scale(4, dpi),
            item.bottom,
        );
        paint::text(
            dc,
            &text,
            label,
            font,
            palette.muted_foreground,
            Align::Left,
        );
        if index + 1 < count {
            let inset = scale(6, dpi);
            paint::fill(
                dc,
                rect(
                    item.right - 1,
                    item.top + inset,
                    item.right,
                    item.bottom - inset,
                ),
                palette.stroke,
            );
        }
    }
    paint::fill(
        dc,
        rect(area.left, area.bottom - 1, area.right, area.bottom),
        palette.stroke,
    );
}

/// A list view in the theme: its colors, its scroll bars and its header. Its rows are painted
/// by `list_rows`, which its parent calls for `NM_CUSTOMDRAW`.
pub fn paint_list(list: HWND) {
    let palette = theme::current();
    list_colors(list, &palette);
    theme_scrollbars(list, &palette);
    let header = unsafe { SendMessageW(list, LVM_GETHEADER, 0, 0) } as HWND;
    if !header.is_null() {
        paint_as(header, Look::Header, Surface::Content);
    }
}

fn list_colors(list: HWND, palette: &Palette) {
    unsafe {
        SendMessageW(
            list,
            LVM_SETBKCOLOR,
            0,
            palette.content_background as LPARAM,
        );
        SendMessageW(
            list,
            LVM_SETTEXTBKCOLOR,
            0,
            palette.content_background as LPARAM,
        );
        SendMessageW(
            list,
            LVM_SETTEXTCOLOR,
            0,
            palette.content_foreground as LPARAM,
        );
    }
}

/// A row's fill and its text color: FastPad's rows, a rounded plate for the selection (dimmer
/// when the list doesn't have the focus) with the accent bar beside it when it does.
pub fn draw_row(
    dc: HDC,
    row: RECT,
    selected: bool,
    focused: bool,
    palette: &Palette,
    dpi: u32,
) -> u32 {
    paint::fill(dc, row, palette.content_background);
    if !selected {
        return palette.content_foreground;
    }
    let plate = rect(
        row.left + scale(ROW_INSET_X, dpi),
        row.top + scale(ROW_INSET_Y, dpi),
        row.right - scale(ROW_INSET_X, dpi),
        row.bottom - scale(ROW_INSET_Y, dpi),
    );
    let fill = if focused || palette.high_contrast {
        palette.selection_background
    } else {
        palette.inactive_selection_background
    };
    let radius = paint::radius(palette, CONTROL_RADIUS, dpi);
    paint::fill_rounded(
        dc,
        plate,
        radius,
        Corners::ALL,
        fill,
        palette.content_background,
    );
    if focused && !palette.high_contrast {
        let room = plate.bottom - plate.top - 2 * radius;
        let height = scale(SELECTION_BAR_HEIGHT, dpi).min(room).max(1);
        let top = plate.top + (plate.bottom - plate.top - height) / 2;
        let bar = rect(
            plate.left,
            top,
            plate.left + scale(SELECTION_BAR_WIDTH, dpi),
            top + height,
        );
        paint::fill_rounded(
            dc,
            bar,
            (bar.right - bar.left) / 2,
            Corners::ALL,
            palette.accent,
            fill,
        );
    }
    palette
        .selection_foreground
        .unwrap_or(palette.hover_foreground)
}

/// The parent's answer to a list view's `NM_CUSTOMDRAW`: it paints each row itself. `tint`
/// may give a cell's text another color (row, column). `None` when `lparam` isn't a list
/// view's custom draw.
pub fn list_rows(lparam: LPARAM, tint: impl Fn(usize, usize) -> Option<u32>) -> Option<LRESULT> {
    let header = unsafe { &*(lparam as *const NMHDR) };
    if header.code != NM_CUSTOMDRAW
        || !class_of(header.hwndFrom).eq_ignore_ascii_case("SysListView32")
    {
        return None;
    }
    let draw = unsafe { &*(lparam as *const NMLVCUSTOMDRAW) };
    let list = header.hwndFrom;
    match draw.nmcd.dwDrawStage {
        CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as LRESULT),
        CDDS_ITEMPREPAINT => {
            let item = draw.nmcd.dwItemSpec;
            paint_row(list, draw.nmcd.hdc, item, &tint);
            Some(CDRF_SKIPDEFAULT as LRESULT)
        }
        _ => Some(CDRF_DODEFAULT as LRESULT),
    }
}

fn paint_row(list: HWND, dc: HDC, item: usize, tint: &impl Fn(usize, usize) -> Option<u32>) {
    let palette = theme::current();
    let dpi = unsafe { GetDpiForWindow(list) };
    let mut bounds = rect(LVIR_BOUNDS as i32, 0, 0, 0);
    unsafe { SendMessageW(list, LVM_GETITEMRECT, item, (&raw mut bounds) as LPARAM) };
    let selected = unsafe { SendMessageW(list, LVM_GETITEMSTATE, item, LVIS_SELECTED as LPARAM) }
        as u32
        & LVIS_SELECTED
        != 0;
    let focused = unsafe { GetFocus() } == list;
    let mut client = rect(0, 0, 0, 0);
    unsafe { GetClientRect(list, &mut client) };
    let row = rect(
        client.left,
        bounds.top,
        client.right.max(bounds.right),
        bounds.bottom,
    );
    let ink = draw_row(dc, row, selected, focused, &palette, dpi);
    let header = unsafe { SendMessageW(list, LVM_GETHEADER, 0, 0) } as HWND;
    let columns = if header.is_null() {
        1
    } else {
        unsafe { SendMessageW(header, HDM_GETITEMCOUNT, 0, 0) }.max(1) as usize
    };
    let font = font_of(list);
    for column in 0..columns {
        // The message reads the column from `top` and what to measure from `left`.
        let mut cell = rect(LVIR_LABEL as i32, column as i32, 0, 0);
        unsafe { SendMessageW(list, LVM_GETSUBITEMRECT, item, (&raw mut cell) as LPARAM) };
        let mut buffer = [0u16; 512];
        let mut text: LVITEMW = unsafe { std::mem::zeroed() };
        text.iSubItem = column as i32;
        text.pszText = buffer.as_mut_ptr();
        text.cchTextMax = buffer.len() as i32;
        let length =
            unsafe { SendMessageW(list, LVM_GETITEMTEXTW, item, (&raw mut text) as LPARAM) }.max(0)
                as usize;
        let label = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
        let color = match tint(item, column) {
            Some(color) if !selected => color,
            _ if selected => ink,
            _ if column > 0 => palette.muted_foreground,
            _ => ink,
        };
        let inset = scale(if column == 0 { 12 } else { 6 }, dpi);
        let area = rect(
            cell.left + inset,
            row.top,
            cell.right - scale(4, dpi),
            row.bottom,
        );
        paint::text(dc, &label, area, font, color, Align::Left);
    }
}

/// Paints one row of an owner-drawn list box (`LBS_OWNERDRAWFIXED | LBS_HASSTRINGS`) for its
/// parent's `WM_DRAWITEM`.
pub fn draw_list_item(item: &DRAWITEMSTRUCT) {
    let palette = theme::current();
    let dpi = unsafe { GetDpiForWindow(item.hwndItem) };
    let selected = item.itemState & ODS_SELECTED != 0;
    let focused = unsafe { GetFocus() } == item.hwndItem || item.itemState & ODS_FOCUS != 0;
    let ink = draw_row(item.hDC, item.rcItem, selected, focused, &palette, dpi);
    if item.itemID == u32::MAX {
        return;
    }
    let index = item.itemID as WPARAM;
    let length = unsafe { SendMessageW(item.hwndItem, LB_GETTEXTLEN, index, 0) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    unsafe {
        SendMessageW(
            item.hwndItem,
            LB_GETTEXT,
            index,
            buffer.as_mut_ptr() as LPARAM,
        )
    };
    let text = String::from_utf16_lossy(&buffer[..length]);
    let area = rect(
        item.rcItem.left + scale(12, dpi),
        item.rcItem.top,
        item.rcItem.right - scale(4, dpi),
        item.rcItem.bottom,
    );
    paint::text(
        item.hDC,
        &text,
        area,
        font_of(item.hwndItem),
        ink,
        Align::Left,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::paint::tests::Canvas;
    use crate::ui::theme::{DARK, LIGHT, System, ThemePreference};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::{
        LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVM_INSERTCOLUMNW,
        LVM_INSERTITEMW, LVM_SETITEMSTATE, LVS_REPORT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BM_CLICK, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_PUSHBUTTON, CB_ADDSTRING,
        CreateWindowExW, DefWindowProcW, DestroyWindow, PRF_CLIENT, RegisterClassW, WM_NOTIFY,
        WM_SETFONT, WNDCLASSW, WS_CHILD, WS_POPUP, WS_VISIBLE,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    extern "system" fn parent_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if let Some(answer) = colors(message, wparam, lparam, Surface::Content) {
            return answer;
        }
        if message == WM_NOTIFY
            && let Some(answer) = list_rows(lparam, |_, _| None)
        {
            return answer;
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// A hidden window holding test controls, in a theme while it lives.
    struct Parent {
        window: HWND,
        font: HFONT,
    }

    impl Parent {
        fn new(preference: ThemePreference) -> Self {
            theme::set_system_for_test(System::default());
            theme::set_preference(preference);
            let class = wide("FastCaptureControlsTest");
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let description = WNDCLASSW {
                lpfnWndProc: Some(parent_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..unsafe { std::mem::zeroed() }
            };
            unsafe { RegisterClassW(&description) };
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    400,
                    300,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            Self {
                window,
                font: ramp::font(ramp::TextStyle::Body, 96),
            }
        }

        fn child(&self, class: &str, text: &str, style: u32, width: i32, height: i32) -> HWND {
            let control = unsafe {
                CreateWindowExW(
                    0,
                    wide(class).as_ptr(),
                    wide(text).as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    width,
                    height,
                    self.window,
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                )
            };
            unsafe { SendMessageW(control, WM_SETFONT, self.font as WPARAM, 0) };
            control
        }
    }

    impl Drop for Parent {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.window);
                windows_sys::Win32::Graphics::Gdi::DeleteObject(self.font as HGDIOBJ);
            }
            theme::set_system_for_test(System::default());
            theme::set_preference(ThemePreference::System);
        }
    }

    /// What `control` paints, `width` by `height`.
    fn printed(control: HWND, width: i32, height: i32) -> Canvas {
        let canvas = Canvas::new(width, height);
        unsafe {
            SendMessageW(
                control,
                WM_PRINTCLIENT,
                canvas.dc as WPARAM,
                PRF_CLIENT as LPARAM,
            )
        };
        canvas
    }

    #[test]
    fn buttons_are_bordered_and_the_default_one_is_the_accent() {
        for (preference, palette) in [
            (ThemePreference::Light, LIGHT),
            (ThemePreference::Dark, DARK),
        ] {
            let parent = Parent::new(preference);
            let plain = parent.child("BUTTON", "Cancel", BS_PUSHBUTTON as u32, 88, 26);
            let primary = parent.child("BUTTON", "Save", BS_DEFPUSHBUTTON as u32, 88, 26);
            paint_as(plain, Look::Button, Surface::Content);
            paint_as(primary, Look::Button, Surface::Content);
            let canvas = printed(plain, 88, 26);
            assert_eq!(canvas.pixel(44, 0), palette.stroke, "{preference:?}");
            assert_eq!(canvas.pixel(4, 13), palette.hover_background);
            assert_eq!(
                canvas.pixel(0, 0),
                palette.content_background,
                "a rounded corner"
            );
            let canvas = printed(primary, 88, 26);
            assert_eq!(canvas.pixel(4, 13), palette.accent, "{preference:?}");
        }
    }

    #[test]
    fn a_check_box_is_a_toggle_and_stays_a_check_box() {
        let parent = Parent::new(ThemePreference::Dark);
        let toggle = parent.child(
            "BUTTON",
            "Start when I sign in",
            BS_AUTOCHECKBOX as u32,
            240,
            26,
        );
        paint_as(toggle, Look::Toggle, Surface::Content);
        let track = toggle_track(rect(0, 0, 240, 26), 96);
        let middle = (track.top + track.bottom) / 2;
        let canvas = printed(toggle, 240, 26);
        assert_eq!(
            canvas.pixel(track.right - 6, middle),
            DARK.content_background,
            "off"
        );
        unsafe { SendMessageW(toggle, BM_CLICK, 0, 0) };
        assert_eq!(
            unsafe { SendMessageW(toggle, BM_GETCHECK, 0, 0) } as u32,
            BST_CHECKED
        );
        let style = unsafe { GetWindowLongW(toggle, GWL_STYLE) } as u32;
        assert_eq!(
            style & BS_TYPEMASK as u32,
            BS_AUTOCHECKBOX as u32,
            "what screen readers read"
        );
        let canvas = printed(toggle, 240, 26);
        assert_eq!(canvas.pixel(track.left + 6, middle), DARK.accent, "on");
    }

    #[test]
    fn radio_buttons_are_segments_with_the_chosen_one_in_the_accent() {
        let parent = Parent::new(ThemePreference::Light);
        let first = parent.child("BUTTON", "Local", BS_AUTORADIOBUTTON as u32, 120, 26);
        let last = parent.child("BUTTON", "Cloud", BS_AUTORADIOBUTTON as u32, 120, 26);
        paint_as(first, Look::Segment(Segment::First), Surface::Content);
        paint_as(last, Look::Segment(Segment::Last), Surface::Content);
        unsafe { SendMessageW(last, BM_SETCHECK, BST_CHECKED as WPARAM, 0) };
        let chosen = printed(last, 120, 26);
        let other = printed(first, 120, 26);
        assert_eq!(chosen.pixel(4, 13), LIGHT.accent);
        assert_eq!(other.pixel(4, 13), LIGHT.hover_background);
        assert_eq!(
            other.pixel(0, 0),
            LIGHT.content_background,
            "the first rounds on the left"
        );
        assert_eq!(
            other.pixel(119, 0),
            LIGHT.stroke,
            "and meets the next one square"
        );
        assert_eq!(
            chosen.pixel(0, 0),
            LIGHT.accent,
            "the last is square on the left"
        );
    }

    #[test]
    fn a_combo_box_is_a_field_with_its_choice() {
        let parent = Parent::new(ThemePreference::Dark);
        let combo = parent.child("COMBOBOX", "", CBS_DROPDOWNLIST as u32, 200, 200);
        let choice = wide("Claude API");
        unsafe {
            SendMessageW(combo, CB_ADDSTRING, 0, choice.as_ptr() as LPARAM);
            SendMessageW(combo, CB_SETCURSEL, 0, 0);
        }
        paint_as(combo, Look::Combo, Surface::Strip);
        let mut client = rect(0, 0, 0, 0);
        unsafe { GetClientRect(combo, &mut client) };
        let canvas = printed(combo, client.right, client.bottom);
        let middle = client.bottom / 2;
        assert_eq!(canvas.pixel(3, middle), DARK.content_background);
        assert_eq!(canvas.pixel(client.right / 2, 0), DARK.stroke);
        assert_eq!(
            canvas.pixel(0, 0),
            DARK.strip_background,
            "its corner blends into the strip"
        );
        let inked = (8..90).any(|x| canvas.pixel(x, middle) != DARK.content_background);
        assert!(inked, "the choice is drawn");
    }

    #[test]
    fn fields_and_labels_take_the_theme_colors() {
        let parent = Parent::new(ThemePreference::Dark);
        let edit = parent.child("EDIT", "text", 0, 200, 26);
        let label = parent.child("STATIC", "Notebook", 0, 100, 20);
        paint_as(edit, Look::Field, Surface::Content);
        unsafe { SendMessageW(edit, WM_NCPAINT, 1, 0) };
        let canvas = Canvas::new(4, 4);
        let answer = colors(
            WM_CTLCOLOREDIT,
            canvas.dc as WPARAM,
            edit as LPARAM,
            Surface::Strip,
        );
        assert_eq!(answer, Some(brush(DARK.content_background) as LRESULT));
        let text = unsafe { windows_sys::Win32::Graphics::Gdi::GetTextColor(canvas.dc) };
        assert_eq!(text, DARK.content_foreground);
        let answer = colors(
            WM_CTLCOLORSTATIC,
            canvas.dc as WPARAM,
            label as LPARAM,
            Surface::Strip,
        );
        assert_eq!(
            answer,
            Some(brush(DARK.strip_background) as LRESULT),
            "a label sits on the strip"
        );
        assert_eq!(colors(WM_PAINT, 0, 0, Surface::Strip), None);
        assert_eq!(brush(DARK.accent), brush(DARK.accent), "brushes are kept");
    }

    #[test]
    fn list_rows_and_the_header_are_painted() {
        let parent = Parent::new(ThemePreference::Light);
        let list = parent.child("SysListView32", "", LVS_REPORT, 300, 120);
        let mut column: LVCOLUMNW = unsafe { std::mem::zeroed() };
        column.mask = LVCF_TEXT | LVCF_WIDTH;
        column.cx = 200;
        let title = wide("Capture");
        column.pszText = title.as_ptr() as *mut u16;
        unsafe { SendMessageW(list, LVM_INSERTCOLUMNW, 0, (&raw const column) as LPARAM) };
        for (index, text) in ["First note", "Second note"].iter().enumerate() {
            let text = wide(text);
            let mut item: LVITEMW = unsafe { std::mem::zeroed() };
            item.mask = LVIF_TEXT;
            item.iItem = index as i32;
            item.pszText = text.as_ptr() as *mut u16;
            unsafe { SendMessageW(list, LVM_INSERTITEMW, 0, (&raw const item) as LPARAM) };
        }
        let mut state: LVITEMW = unsafe { std::mem::zeroed() };
        state.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
        state.state = LVIS_SELECTED | LVIS_FOCUSED;
        unsafe { SendMessageW(list, LVM_SETITEMSTATE, 1, (&raw const state) as LPARAM) };
        paint_list(list);
        let mut row = rect(LVIR_BOUNDS as i32, 0, 0, 0);
        unsafe { SendMessageW(list, LVM_GETITEMRECT, 1, (&raw mut row) as LPARAM) };
        let canvas = printed(list, 300, 120);
        let middle = (row.top + row.bottom) / 2;
        assert_eq!(
            canvas.pixel(250, middle),
            LIGHT.inactive_selection_background,
            "the list hasn't the focus"
        );
        assert_eq!(
            canvas.pixel(1, middle),
            LIGHT.content_background,
            "the plate is inset"
        );

        let header = unsafe { SendMessageW(list, LVM_GETHEADER, 0, 0) } as HWND;
        let mut area = rect(0, 0, 0, 0);
        unsafe { GetClientRect(header, &mut area) };
        let canvas = printed(header, area.right, area.bottom);
        assert_eq!(canvas.pixel(area.right - 2, 2), LIGHT.content_background);
        assert_eq!(canvas.pixel(area.right / 2, area.bottom - 1), LIGHT.stroke);
    }

    #[test]
    fn high_contrast_keeps_corners_square_and_windows_colors() {
        let parent = Parent::new(ThemePreference::Dark);
        theme::set_system_for_test(System {
            dark: true,
            high_contrast: true,
        });
        let button = parent.child("BUTTON", "OK", BS_PUSHBUTTON as u32, 88, 26);
        paint_as(button, Look::Button, Surface::Content);
        let canvas = printed(button, 88, 26);
        let palette = theme::current();
        assert!(palette.high_contrast);
        assert_eq!(canvas.pixel(0, 0), palette.stroke, "square, unblended");
    }
}
