//! Engine and window-system hooks. Windows (WebView2 + Win32) gets the full
//! transport layer and the custom title bar; other platforms rely on the
//! navigation + runtime layers and the compositor's own decorations.

/// Window-frame events the OS reports from outside Slint's view.
#[derive(Debug, Clone, Copy)]
pub enum ChromeEvent {
    /// Pointer over the maximize cell (arrives as non-client messages).
    MaxHot(bool),
    /// Window gained / lost the keyboard.
    Active(bool),
    Maximized(bool),
}

/// How a popup window behaves.
#[derive(Debug, Clone, Copy)]
pub enum PopupKind {
    /// Takes the keyboard; closes when it loses it.
    Menu,
    /// Hover card: never takes the keyboard.
    Card,
    /// Toast: never takes the keyboard, but its action can be clicked.
    Toast,
}

#[cfg(windows)]
mod popup;
#[cfg(windows)]
mod titlebar;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::popup::{activate, client_origin, own_popup};
#[cfg(windows)]
pub use self::titlebar::{install_titlebar, is_maximized, set_hitmap, style_frame, toggle_maximize, HitMap};
#[cfg(windows)]
pub use self::windows::*;

/// Whether the bar draws its own window controls.
pub const CUSTOM_TITLEBAR: bool = cfg!(windows);

#[cfg(not(windows))]
mod fallback {
    use super::ChromeEvent;
    use crate::shield::Shield;
    use std::rc::Rc;

    pub const BROWSER_ARGS: &str = "";

    #[derive(Clone, Copy, Debug, Default)]
    pub struct HitMap {
        pub scale: f32,
        pub bar_h: f32,
        pub drags: [(f32, f32); 2],
        pub max_btn: (f32, f32),
    }

    pub fn install_transport(_wv: &wry::WebView, _tab: u64, _shield: Rc<Shield>) -> Result<(), ()> {
        Ok(())
    }
    pub fn install_accelerators(_wv: &wry::WebView, _on_key: impl Fn(String) + 'static) -> Result<(), ()> {
        Ok(())
    }
    pub fn install_focus(_wv: &wry::WebView, _on_focus: impl Fn() + 'static) -> Result<(), ()> {
        Ok(())
    }
    pub fn set_background(_wv: &wry::WebView, _background: bool) {}
    pub fn set_web_theme(_wv: &wry::WebView, _dark: bool) {}
    // Under X11/Wayland the compositor decorates (or, on Hyprland/Sway, tiles) the window.
    pub fn install_titlebar(_hwnd: isize, _sink: impl Fn(ChromeEvent) + 'static) -> bool {
        false
    }
    pub fn set_hitmap(_h: HitMap) {}
    pub fn style_frame(_hwnd: isize, _dark: bool, _border: u32) {}
    pub fn toggle_maximize(_hwnd: isize) {}
    pub fn is_maximized(_hwnd: isize) -> bool {
        false
    }
    pub fn own_popup(_popup: isize, _owner: isize, _kind: super::PopupKind, _on_deactivate: impl Fn() + 'static) {}
    pub fn install_media(_wv: &wry::WebView, _on_audio: impl Fn(bool) + 'static, _on_crash: impl Fn(bool) + 'static) -> Result<(), ()> {
        Ok(())
    }
    pub fn set_muted(_wv: &wry::WebView, _muted: bool) {}
    pub fn try_suspend(_wv: &wry::WebView) {}
    pub fn set_clipboard(_text: &str) -> bool {
        false
    }
    pub fn client_origin(_hwnd: isize) -> (i32, i32) {
        (0, 0)
    }
    pub fn activate(_hwnd: isize) {}
}
#[cfg(not(windows))]
pub use fallback::*;
