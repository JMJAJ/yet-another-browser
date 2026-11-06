//! Engine-specific hooks. Windows (WebView2) gets the full transport layer;
//! other engines currently rely on the navigation + runtime layers.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(not(windows))]
mod fallback {
    use crate::shield::Shield;
    use std::rc::Rc;

    pub const BROWSER_ARGS: &str = "";

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
    pub fn set_titlebar_dark(_hwnd: isize, _dark: bool) {}
}
#[cfg(not(windows))]
pub use fallback::*;
