//! Owned popup windows (menus now; hover cards and toasts later).
//!
//! The page is a native child window, so anything that must appear over it
//! has to be its own top-level window. An *owned* window stays above its
//! owner, minimizes with it, and stays out of the taskbar and alt-tab when it
//! is also a tool window.

use super::PopupKind;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWM_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE, SW_HIDE, SW_SHOW,
    SW_SHOWNOACTIVATE, WA_INACTIVE, WM_ACTIVATE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

thread_local! {
    static ON_DEACTIVATE: RefCell<HashMap<isize, Box<dyn Fn()>>> = RefCell::new(HashMap::new());
}

const SUBCLASS_ID: usize = 0xB2B6;

/// Make `popup` an owned tool window of `owner`. Menus take the keyboard and
/// `on_deactivate` runs when they lose it (outside click, alt-tab, the
/// owner being clicked); cards and toasts never take it.
pub fn own_popup(popup: isize, owner: isize, kind: PopupKind, on_deactivate: impl Fn() + 'static) {
    let w = HWND(popup as *mut _);
    let passive = !matches!(kind, PopupKind::Menu);
    unsafe {
        SetWindowLongPtrW(w, GWLP_HWNDPARENT, owner);
        let mut ex = (GetWindowLongPtrW(w, GWL_EXSTYLE) | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        if passive {
            ex |= WS_EX_NOACTIVATE.0 as isize;
        }
        SetWindowLongPtrW(w, GWL_EXSTYLE, ex);
        let pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(w, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, 4);
        // the taskbar only re-reads the style when the window is shown again
        let _ = ShowWindow(w, SW_HIDE);
        let _ = ShowWindow(w, if passive { SW_SHOWNOACTIVATE } else { SW_SHOW });
        if passive {
            // its very first show (before these styles) took focus: give it back
            let _ = SetForegroundWindow(HWND(owner as *mut _));
            return;
        }
        // hook deactivation only now, so the hide/show above cannot close it
        let _ = SetWindowSubclass(w, Some(popup_proc), SUBCLASS_ID, 0);
    }
    ON_DEACTIVATE.with(|m| m.borrow_mut().insert(popup, Box::new(on_deactivate)));
}

/// Screen position of a window's client origin, in physical pixels.
pub fn client_origin(h: isize) -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = ClientToScreen(HWND(h as *mut _), &mut p);
    }
    (p.x, p.y)
}

pub fn activate(h: isize) {
    unsafe {
        let _ = SetForegroundWindow(HWND(h as *mut _));
    }
}

unsafe extern "system" fn popup_proc(w: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if msg == WM_ACTIVATE && (wp.0 & 0xffff) as u32 == WA_INACTIVE {
            ON_DEACTIVATE.with(|m| {
                if let Some(f) = m.borrow().get(&(w.0 as isize)) {
                    f()
                }
            });
        }
        DefSubclassProc(w, msg, wp, lp)
    }
}
