//! The live text bubble on Windows: a layered, click-through, never-focused
//! popup drawn with GDI, so text shaping (Devanagari, Arabic...) and font
//! fallback come from the system. Lives on the tray's thread; tao's message
//! loop delivers its messages.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreateRoundRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromWindow, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, HDC, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetForegroundWindow,
    GetWindowLongPtrW, KillTimer, RegisterClassW, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, GWLP_USERDATA, HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST,
    WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::{ellipsized, meter_level, tail_start, Phase};
use crate::audio::LiveTap;
use crate::error::{Result, VoxError};

const CLASS_NAME: &str = "VoxLiveText";
const TIMER_ID: usize = 1;
/// Meter and animation refresh.
const FRAME: Duration = Duration::from_millis(50);
/// How long a message stays up.
const MESSAGE_FOR: Duration = Duration::from_millis(2200);

// Layout at 96 DPI; scaled by the monitor's DPI.
const WIDTH: i32 = 520;
const PAD_X: i32 = 16;
const PAD_Y: i32 = 11;
const HEADER: i32 = 20;
const GAP: i32 = 4;
const BODY_LINES: i32 = 2;
const RADIUS: i32 = 16;
/// Distance from the bottom of the work area (above the taskbar).
const MARGIN_BOTTOM: i32 = 56;
const BARS: usize = 5;
const BAR_WIDTH: i32 = 3;
const BAR_GAP: i32 = 3;
const TITLE_PX: i32 = 12;
const BODY_PX: i32 = 15;
const OPACITY: u8 = 242;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}
const BACKGROUND: COLORREF = rgb(0x1F, 0x1F, 0x22);
const TITLE: COLORREF = rgb(0xA8, 0xA8, 0xB0);
const SETTLED: COLORREF = rgb(0xF4, 0xF4, 0xF6);
const PENDING: COLORREF = rgb(0x96, 0x96, 0xA0);
const LISTEN: COLORREF = rgb(0xFF, 0x5A, 0x5F);
const EDIT: COLORREF = rgb(0x6E, 0xA8, 0xFF);
const IDLE_BAR: COLORREF = rgb(0x5A, 0x5A, 0x62);

/// Bar heights relative to the level, center tallest.
const BAR_WEIGHTS: [f32; BARS] = [0.55, 0.8, 1.0, 0.8, 0.55];

struct Fonts {
    dpi: u32,
    title: HFONT,
    body: HFONT,
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.title);
            DeleteObject(self.body);
        }
    }
}

struct State {
    phase: Phase,
    title: String,
    settled: String,
    pending: String,
    tap: Option<LiveTap>,
    level: f32,
    frame: u32,
    hide_at: Option<Instant>,
    visible: bool,
    fonts: Option<Fonts>,
    /// Client size the window was last laid out at.
    size: (i32, i32),
}

pub struct Window {
    hwnd: HWND,
    /// Owned here; the window procedure borrows it through GWLP_USERDATA.
    state: *mut RefCell<State>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Window {
    pub fn new() -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS_NAME);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // Fails harmlessly if the class already exists (a second window).
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                WIDTH,
                PAD_Y * 2 + HEADER,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(VoxError::Other("CreateWindowExW failed".into()));
            }
            SetLayeredWindowAttributes(hwnd, 0, OPACITY, LWA_ALPHA);
            let state = Box::into_raw(Box::new(RefCell::new(State {
                phase: Phase::Listening,
                title: String::new(),
                settled: String::new(),
                pending: String::new(),
                tap: None,
                level: 0.0,
                frame: 0,
                hide_at: None,
                visible: false,
                fonts: None,
                size: (0, 0),
            })));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            Ok(Self { hwnd, state })
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> Option<R> {
        // SAFETY: `state` lives as long as `self`; the window procedure only
        // borrows it while handling a message, never across these calls.
        let cell = unsafe { &*self.state };
        cell.try_borrow_mut().ok().map(|mut s| f(&mut s))
    }

    pub fn show(&mut self, phase: Phase, title: &str, tap: Option<LiveTap>) {
        self.with_state(|s| {
            s.phase = phase;
            s.title = title.to_string();
            s.settled.clear();
            s.pending.clear();
            s.tap = tap;
            s.level = 0.0;
            s.hide_at = None;
            s.visible = true;
        });
        self.layout(true);
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(
                self.hwnd,
                TIMER_ID,
                FRAME.as_millis() as u32,
                None,
            );
        }
    }

    pub fn set_text(&mut self, settled: &str, pending: &str) {
        let changed = self.with_state(|s| {
            let changed = s.settled != settled || s.pending != pending;
            s.settled = settled.to_string();
            s.pending = pending.to_string();
            changed && s.visible
        });
        if changed == Some(true) {
            self.layout(false);
        }
    }

    pub fn set_phase(&mut self, phase: Phase, title: &str) {
        let visible = self.with_state(|s| {
            s.phase = phase;
            s.title = title.to_string();
            if phase != Phase::Listening && phase != Phase::ListeningEdit {
                s.tap = None;
            }
            s.hide_at = (phase == Phase::Message).then(|| Instant::now() + MESSAGE_FOR);
            if phase == Phase::Message {
                s.visible = true;
            }
            s.visible
        });
        if visible == Some(true) {
            if phase == Phase::Message {
                self.layout(true);
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(
                        self.hwnd,
                        TIMER_ID,
                        FRAME.as_millis() as u32,
                        None,
                    );
                }
            } else {
                self.layout(false);
            }
        }
    }

    pub fn hide(&mut self) {
        self.with_state(|s| {
            s.visible = false;
            s.tap = None;
            s.hide_at = None;
        });
        unsafe {
            KillTimer(self.hwnd, TIMER_ID);
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Sizes the bubble for its text and puts it at the bottom center of
    /// the monitor the user is working on (`reposition`), or keeps its
    /// bottom edge where it is while it grows.
    fn layout(&mut self, reposition: bool) {
        unsafe {
            let hwnd = self.hwnd;
            let monitor = MonitorFromWindow(
                if reposition {
                    GetForegroundWindow()
                } else {
                    hwnd
                },
                MONITOR_DEFAULTTONEAREST,
            );
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return;
            }
            let work = info.rcWork;
            if reposition {
                // Move onto that monitor first so the DPI read below is its.
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    work.left,
                    work.top,
                    0,
                    0,
                    SWP_NOACTIVATE,
                );
            }
            let dpi = GetDpiForWindow(hwnd).max(96);
            let scale = |v: i32| v * dpi as i32 / 96;
            let width = scale(WIDTH).min(work.right - work.left - scale(32));

            let Some(height) = self.with_state(|s| {
                ensure_fonts(s, dpi);
                let body = body_height(s, width - 2 * scale(PAD_X), dpi);
                let height =
                    scale(PAD_Y) * 2 + scale(HEADER) + if body > 0 { scale(GAP) + body } else { 0 };
                s.size = (width, height);
                height
            }) else {
                return;
            };
            let x = work.left + (work.right - work.left - width) / 2;
            let y = work.bottom - scale(MARGIN_BOTTOM) - height;
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let r = scale(RADIUS);
            // The system owns the region from here on.
            SetWindowRgn(
                hwnd,
                CreateRoundRectRgn(0, 0, width + 1, height + 1, r, r),
                1,
            );
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.hwnd, TIMER_ID);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.state));
        }
    }
}

fn ensure_fonts(s: &mut State, dpi: u32) {
    if s.fonts.as_ref().is_some_and(|f| f.dpi == dpi) {
        return;
    }
    let face = wide("Segoe UI");
    let make = |px: i32, weight: i32| unsafe {
        CreateFontW(
            -(px * dpi as i32 / 96),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            face.as_ptr(),
        )
    };
    s.fonts = Some(Fonts {
        dpi,
        title: make(TITLE_PX, 600),
        body: make(BODY_PX, 400),
    });
}

/// The body text as shown: the newest words that fit in two lines, and how
/// much of that is settled (drawn brighter) rather than still changing.
fn visible_body(s: &State, fits: impl Fn(&str) -> bool) -> (String, String) {
    let full = match (s.settled.is_empty(), s.pending.is_empty()) {
        (true, true) => return (String::new(), String::new()),
        (_, true) => s.settled.clone(),
        (true, false) => s.pending.clone(),
        (false, false) => format!("{} {}", s.settled, s.pending),
    };
    let start = tail_start(&full, fits);
    let shown = ellipsized(&full, start);
    let settled = if start < s.settled.len() {
        ellipsized(&s.settled, start)
    } else {
        String::new()
    };
    (shown, settled)
}

/// Height of a word-wrapped text in the body font, measured on a scratch DC.
///
/// Never pass DrawTextW an empty string: an empty Vec's pointer is a
/// placeholder (0x2), and USER32 reads it even for a count of 0, which
/// crashed the tray with an access violation inside the window procedure.
unsafe fn measure(hdc: HDC, text: &str, width: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let w: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: 0,
    };
    DrawTextW(
        hdc,
        w.as_ptr(),
        w.len() as i32,
        &mut rc,
        DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
    );
    rc.bottom - rc.top
}

fn line_height(dpi: u32) -> i32 {
    // Segoe UI's line spacing is about 1.33x its pixel size.
    (BODY_PX * dpi as i32 / 96) * 4 / 3
}

fn body_height(s: &State, width: i32, dpi: u32) -> i32 {
    let Some(fonts) = s.fonts.as_ref() else {
        return 0;
    };
    unsafe {
        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        let old = SelectObject(hdc, fonts.body);
        let max = line_height(dpi) * BODY_LINES + 2;
        let (shown, _) = visible_body(s, |t| measure(hdc, t, width) <= max);
        let h = if shown.is_empty() {
            0
        } else {
            measure(hdc, &shown, width).min(max)
        };
        SelectObject(hdc, old);
        DeleteDC(hdc);
        h
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Clicks go to whatever is underneath; the bubble is never focused.
        WM_NCHITTEST => return HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => return 1,
        WM_TIMER | WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
            if !ptr.is_null() {
                if let Ok(mut s) = (*ptr).try_borrow_mut() {
                    if msg == WM_TIMER {
                        tick(hwnd, &mut s);
                    } else if s.visible {
                        paint(hwnd, &mut s);
                        return 0;
                    } else {
                        // Hidden (a paint can arrive right after creation):
                        // nothing to draw, just mark it painted.
                        let mut ps: PAINTSTRUCT = std::mem::zeroed();
                        BeginPaint(hwnd, &mut ps);
                        EndPaint(hwnd, &ps);
                        return 0;
                    }
                    return 0;
                }
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Advances the meter, and hides a message whose time is up.
unsafe fn tick(hwnd: HWND, s: &mut State) {
    if s.hide_at.is_some_and(|at| Instant::now() >= at) {
        s.visible = false;
        s.hide_at = None;
        KillTimer(hwnd, TIMER_ID);
        ShowWindow(hwnd, SW_HIDE);
        return;
    }
    s.frame = s.frame.wrapping_add(1);
    if let Some(tap) = s.tap.as_ref() {
        let window = (tap.sample_rate() / 20) as usize; // 50 ms
        let len = tap.len();
        let mut recent = Vec::with_capacity(window);
        tap.read_from(len.saturating_sub(window), &mut recent);
        let target = meter_level(&recent);
        // Rise at once, fall gently, like a VU meter.
        s.level = if target > s.level {
            target
        } else {
            s.level * 0.8 + target * 0.2
        };
    }
    InvalidateRect(hwnd, std::ptr::null(), 0);
}

unsafe fn paint(hwnd: HWND, s: &mut State) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let (w, h) = (client.right, client.bottom);
    let dpi = GetDpiForWindow(hwnd).max(96);
    ensure_fonts(s, dpi);
    let scale = |v: i32| v * dpi as i32 / 96;

    // Draw off-screen, then copy, so the meter never flickers.
    let mem = CreateCompatibleDC(hdc);
    let bitmap = CreateCompatibleBitmap(hdc, w, h);
    let old_bitmap = SelectObject(mem, bitmap);
    let background = CreateSolidBrush(BACKGROUND);
    FillRect(mem, &client, background);
    DeleteObject(background);

    // Level meter, vertically centered in the header row.
    let header_top = scale(PAD_Y);
    let header_mid = header_top + scale(HEADER) / 2;
    let max_bar = scale(HEADER) - scale(4);
    let min_bar = scale(3);
    let mut x = scale(PAD_X);
    for (i, weight) in BAR_WEIGHTS.iter().enumerate() {
        let (height, color) = match s.phase {
            Phase::Listening | Phase::ListeningEdit => {
                // A little independent wobble per bar so speech looks alive.
                let wobble = 0.75 + 0.25 * ((s.frame as f32 * 0.9 + i as f32 * 1.7).sin());
                let h = (s.level * weight * wobble * max_bar as f32) as i32;
                let color = if s.phase == Phase::ListeningEdit {
                    EDIT
                } else {
                    LISTEN
                };
                (h.max(min_bar), color)
            }
            Phase::Working => {
                // A light sweeping across the bars.
                let lit = (s.frame / 3) as usize % BARS == i;
                (
                    if lit { min_bar * 2 } else { min_bar },
                    if lit { TITLE } else { IDLE_BAR },
                )
            }
            Phase::Message => (min_bar, IDLE_BAR),
        };
        let bar = RECT {
            left: x,
            top: header_mid - height / 2,
            right: x + scale(BAR_WIDTH),
            bottom: header_mid - height / 2 + height,
        };
        let brush = CreateSolidBrush(color);
        FillRect(mem, &bar, brush);
        DeleteObject(brush);
        x += scale(BAR_WIDTH) + scale(BAR_GAP);
    }

    SetBkMode(mem, TRANSPARENT as i32);
    if let Some(fonts) = s.fonts.as_ref() {
        // Title, to the right of the meter.
        let old_font = SelectObject(mem, fonts.title);
        SetTextColor(mem, TITLE);
        let title: Vec<u16> = s.title.encode_utf16().collect();
        let mut rc = RECT {
            left: x + scale(8),
            top: header_top,
            right: w - scale(PAD_X),
            bottom: header_top + scale(HEADER),
        };
        if !title.is_empty() {
            DrawTextW(
                mem,
                title.as_ptr(),
                title.len() as i32,
                &mut rc,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }

        // Body: everything in the dimmer color, then the settled part over
        // it in the bright one. Word wrapping is greedy, so the settled
        // prefix wraps exactly as it does inside the whole text.
        SelectObject(mem, fonts.body);
        let width = w - 2 * scale(PAD_X);
        let max = line_height(dpi) * BODY_LINES + 2;
        let (shown, settled) = visible_body(s, |t| measure(mem, t, width) <= max);
        let body_top = header_top + scale(HEADER) + scale(GAP);
        for (text, color) in [(&shown, PENDING), (&settled, SETTLED)] {
            if text.is_empty() {
                continue;
            }
            let wtext: Vec<u16> = text.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(PAD_X),
                top: body_top,
                right: w - scale(PAD_X),
                bottom: h - scale(PAD_Y) + scale(2),
            };
            SetTextColor(mem, color);
            DrawTextW(
                mem,
                wtext.as_ptr(),
                wtext.len() as i32,
                &mut rc,
                DT_WORDBREAK | DT_NOPREFIX,
            );
        }
        SelectObject(mem, old_font);
    }

    BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bitmap);
    DeleteObject(bitmap);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}
