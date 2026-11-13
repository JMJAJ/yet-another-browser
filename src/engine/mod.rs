//! The page engine behind each tab. `Page` is what the app holds; it hides
//! whether a tab is drawn by the system WebView2 (via wry) or by bundled
//! Chromium (CEF, `--features engine-cef`).

use crate::platform;
use wry::Rect;

#[cfg(feature = "engine-cef")]
pub mod cef;

pub enum Page {
    Wry(wry::WebView),
    #[cfg(feature = "engine-cef")]
    Cef(cef::Handle),
}

/// Which engine new tabs use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    WebView2,
    #[cfg(feature = "engine-cef")]
    Chromium,
}

impl Kind {
    /// CEF when it is compiled in, unless `BRX_ENGINE=webview2` asks
    /// otherwise (used to compare the two engines).
    #[cfg_attr(not(feature = "engine-cef"), allow(dead_code))]
    pub fn pick() -> Kind {
        #[cfg(feature = "engine-cef")]
        {
            if std::env::var("BRX_ENGINE").map_or(true, |v| v != "webview2") {
                return Kind::Chromium;
            }
        }
        Kind::WebView2
    }
}

impl Page {
    pub fn load_url(&self, url: &str) {
        match self {
            Page::Wry(w) => {
                let _ = w.load_url(url);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.load_url(url),
        }
    }

    pub fn load_html(&self, html: &str) {
        match self {
            Page::Wry(w) => {
                let _ = w.load_html(html);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.load_html(html),
        }
    }

    pub fn find(&self, text: &str, forward: bool, case: bool, next: bool) {
        match self {
            Page::Wry(w) => {
                // WebView2 has no find API in wry: window.find, without counts
                let js = format!(
                    "window.find({}, {case}, {}, true)",
                    serde_json::to_string(text).unwrap_or_default(),
                    !forward
                );
                if !text.is_empty() && next || !text.is_empty() {
                    let _ = w.evaluate_script(&js);
                }
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.find(text, forward, case, next),
        }
    }

    pub fn stop_find(&self, keep_selection: bool) {
        match self {
            Page::Wry(_) => {}
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.stop_find(keep_selection),
        }
        let _ = keep_selection;
    }

    pub fn set_zoom(&self, percent: u32) {
        match self {
            Page::Wry(w) => {
                let _ = w.zoom(percent as f64 / 100.0);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.set_zoom(percent),
        }
    }

    pub fn devtools(&self) {
        match self {
            Page::Wry(_) => {}
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.devtools(),
        }
    }

    /// cut / copy / paste / selectall / source
    pub fn edit(&self, what: &'static str) {
        match self {
            Page::Wry(w) => {
                let js = match what {
                    "selectall" => "document.execCommand('selectAll')".to_string(),
                    "source" => return,
                    other => format!("document.execCommand('{other}')"),
                };
                let _ = w.evaluate_script(&js);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.edit(what),
        }
    }

    /// Run script in the page's main frame.
    pub fn run_script(&self, js: &str) {
        match self {
            Page::Wry(w) => {
                let _ = w.evaluate_script(js);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.run_script(js),
        }
    }

    pub fn reload(&self) {
        match self {
            Page::Wry(w) => {
                let _ = w.reload();
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.reload(),
        }
    }

    pub fn stop(&self) {
        match self {
            Page::Wry(w) => {
                let _ = w.evaluate_script("window.stop()");
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.stop(),
        }
    }

    pub fn go_back(&self) {
        match self {
            Page::Wry(w) => {
                let _ = w.go_back();
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.go_back(),
        }
    }

    pub fn go_forward(&self) {
        match self {
            Page::Wry(w) => {
                let _ = w.go_forward();
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.go_forward(),
        }
    }

    pub fn can_go_back(&self) -> bool {
        match self {
            Page::Wry(w) => w.can_go_back().unwrap_or(false),
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.state().can_back,
        }
    }

    pub fn can_go_forward(&self) -> bool {
        match self {
            Page::Wry(w) => w.can_go_forward().unwrap_or(false),
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.state().can_forward,
        }
    }

    pub fn set_visible(&self, on: bool) {
        match self {
            Page::Wry(w) => {
                let _ = w.set_visible(on);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.set_visible(on),
        }
    }

    /// `rect` in logical px; `scale` converts to the physical px CEF uses.
    pub fn set_bounds(&self, rect: Rect, scale: f64) {
        match self {
            Page::Wry(w) => {
                let _ = w.set_bounds(rect);
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.set_bounds(rect, scale),
        }
        let _ = scale;
    }

    pub fn focus(&self) {
        match self {
            Page::Wry(w) => {
                let _ = w.focus();
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.focus(),
        }
    }

    /// Hand keyboard focus back to the chrome.
    pub fn focus_parent(&self) {
        match self {
            Page::Wry(w) => {
                let _ = w.focus_parent();
            }
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.focus_parent(),
        }
    }

    /// A hidden tab: release memory, slow timers.
    pub fn set_background(&self, background: bool) {
        match self {
            Page::Wry(w) => platform::set_background(w, background),
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.set_background(background),
        }
    }

    /// `prefers-color-scheme` for pages.
    pub fn set_dark(&self, dark: bool) {
        match self {
            Page::Wry(w) => platform::set_web_theme(w, dark),
            #[cfg(feature = "engine-cef")]
            Page::Cef(_) => {}
        }
        let _ = dark;
    }

    pub fn set_muted(&self, muted: bool) {
        match self {
            Page::Wry(w) => platform::set_muted(w, muted),
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.set_muted(muted),
        }
    }

    /// Put a long-hidden tab to sleep.
    pub fn suspend(&self) {
        match self {
            Page::Wry(w) => platform::try_suspend(w),
            #[cfg(feature = "engine-cef")]
            Page::Cef(c) => c.set_background(true),
        }
    }
}
