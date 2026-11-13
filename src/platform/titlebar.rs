//! Custom title bar on Windows.
//!
//! The native frame is kept (so Aero Snap, resize borders, the minimize
//! animation and Win+arrow keep working) and only its caption is removed:
//! `WM_NCCALCSIZE` hands the caption's rows to the client area, and
//! `WM_NCHITTEST` tells Windows which parts of our bar behave like a
//! caption (drag, double-click, system menu) and which cell is the maximize
//! button (that answer is what enables the Windows 11 snap-layout flyout).

use super::ChromeEvent;
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, IsZoomed, SetWindowPos, ShowWindow, HTCAPTION, HTCLIENT, HTMAXBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    NCCALCSIZE_PARAMS, SIZE_MAXIMIZED, SM_CXPADDEDBORDER, SM_CYFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE, WA_INACTIVE, WM_ACTIVATE, WM_MOUSEMOVE, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SIZE,
};

/// Bar regions in logical pixels, exported by the Slint layout.
#[derive(Clone, Copy, Debug)]
pub struct HitMap {
    pub scale: f32,
    pub bar_h: f32,
    /// (x, width) of each empty drag region.
    pub drags: [(f32, f32); 2],
    /// (x, width) of the maximize cell; width 0 = no window controls.
    pub max_btn: (f32, f32),
}

impl Default for HitMap {
    fn default() -> Self {
        HitMap { scale: 1.0, bar_h: 32.0, drags: [(0.0, 0.0); 2], max_btn: (0.0, 0.0) }
    }
}

thread_local! {
    static HIT: Cell<HitMap> = Cell::new(HitMap::default());
    static SINK: RefCell<Option<Box<dyn Fn(ChromeEvent)>>> = const { RefCell::new(None) };
    static MAX_HOT: Cell<bool> = const { Cell::new(false) };
}

const SUBCLASS_ID: usize = 0xB2B5;

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

fn emit(e: ChromeEvent) {
    SINK.with(|s| {
        if let Some(f) = s.borrow().as_ref() {
            f(e)
        }
    });
}

pub fn set_hitmap(h: HitMap) {
    HIT.with(|c| c.set(h));
}

pub fn install_titlebar(h: isize, sink: impl Fn(ChromeEvent) + 'static) -> bool {
    SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
    unsafe {
        let w = hwnd(h);
        if !SetWindowSubclass(w, Some(subclass_proc), SUBCLASS_ID, 0).as_bool() {
            return false;
        }
        // Windows 11 rounds top-level windows; the design is 0px radius.
        let pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            w,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
        // Recalculate the frame now that WM_NCCALCSIZE answers differently.
        let _ = SetWindowPos(w, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    true
}

/// Dark-mode hint for system menus and scrollbars, plus the 1px window
/// border in the theme's border colour (Windows 11; ignored on 10).
pub fn style_frame(h: isize, dark: bool, border_rgb: u32) {
    let w = hwnd(h);
    let dark = dark as i32;
    // COLORREF is 0x00BBGGRR
    let c = COLORREF(((border_rgb & 0xff) << 16) | (border_rgb & 0xff00) | ((border_rgb >> 16) & 0xff));
    unsafe {
        let _ = DwmSetWindowAttribute(w, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const i32 as *const _, 4);
        let _ = DwmSetWindowAttribute(w, DWMWA_BORDER_COLOR, &c as *const COLORREF as *const _, 4);
    }
}

pub fn toggle_maximize(h: isize) {
    unsafe {
        let w = hwnd(h);
        let _ = ShowWindow(w, if IsZoomed(w).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
    }
}

pub fn is_maximized(h: isize) -> bool {
    unsafe { IsZoomed(hwnd(h)).as_bool() }
}

fn set_hot(w: HWND, on: bool) {
    if MAX_HOT.with(|c| c.replace(on)) == on {
        return;
    }
    if on {
        // ask for WM_NCMOUSELEAVE so the highlight clears when the pointer leaves
        let mut t = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE | TME_NONCLIENT,
            hwndTrack: w,
            dwHoverTime: 0,
        };
        unsafe {
            let _ = TrackMouseEvent(&mut t);
        }
    }
    emit(ChromeEvent::MaxHot(on));
}

fn lresult(v: u32) -> LRESULT {
    LRESULT(v as isize)
}

unsafe extern "system" fn subclass_proc(w: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCALCSIZE if wp.0 != 0 => {
                let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
                let orig = params.rgrc[0];
                let r = DefSubclassProc(w, msg, wp, lp);
                // Keep the native left/right/bottom frame (invisible resize
                // borders on Windows 10/11); give the caption rows to us.
                // Maximized windows hang off-screen by the frame width, so
                // inset the top by the same amount or the bar is clipped.
                let inset = params.rgrc[0].left - orig.left;
                params.rgrc[0].top = if IsZoomed(w).as_bool() { orig.top + inset.max(0) } else { orig.top };
                return r;
            }
            WM_NCHITTEST => {
                let r = DefSubclassProc(w, msg, wp, lp);
                if r.0 as u32 != HTCLIENT {
                    return r;
                }
                let mut pt = POINT { x: (lp.0 & 0xffff) as i16 as i32, y: ((lp.0 >> 16) & 0xffff) as i16 as i32 };
                let _ = ScreenToClient(w, &mut pt);
                let zoomed = IsZoomed(w).as_bool();
                let dpi = GetDpiForWindow(w);
                let frame = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                // The top frame is now client area: rebuild the resize band.
                if !zoomed && pt.y < frame {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(w, &mut rc);
                    let edge = if pt.x < frame * 2 {
                        HTTOPLEFT
                    } else if pt.x >= rc.right - frame * 2 {
                        HTTOPRIGHT
                    } else {
                        HTTOP
                    };
                    return lresult(edge);
                }
                let h = HIT.with(|c| c.get());
                let s = if h.scale > 0.0 { h.scale } else { 1.0 };
                let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
                if y < h.bar_h {
                    let (mx, mw) = h.max_btn;
                    if mw > 0.0 && x >= mx && x < mx + mw {
                        return lresult(HTMAXBUTTON);
                    }
                    if h.drags.iter().any(|&(dx, dw)| dw > 0.0 && x >= dx && x < dx + dw) {
                        return lresult(HTCAPTION);
                    }
                }
                return r;
            }
            WM_NCMOUSEMOVE => set_hot(w, wp.0 as u32 == HTMAXBUTTON),
            WM_NCMOUSELEAVE | WM_MOUSEMOVE => set_hot(w, false),
            // Our cell draws the button: keep Windows from drawing and
            // tracking a classic one, and act on release like a real button.
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wp.0 as u32 == HTMAXBUTTON => return LRESULT(0),
            WM_NCLBUTTONUP if wp.0 as u32 == HTMAXBUTTON => {
                let _ = ShowWindow(w, if IsZoomed(w).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                return LRESULT(0);
            }
            WM_ACTIVATE => emit(ChromeEvent::Active((wp.0 & 0xffff) as u32 != WA_INACTIVE)),
            WM_SIZE => emit(ChromeEvent::Maximized(wp.0 as u32 == SIZE_MAXIMIZED)),
            _ => {}
        }
        DefSubclassProc(w, msg, wp, lp)
    }
}
