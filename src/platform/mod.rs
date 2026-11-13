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

/// Local wall-clock time of an event (history entries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl Stamp {
    fn days(&self) -> i64 {
        // days since 1970-01-01 (proleptic Gregorian), Howard Hinnant's algorithm
        let (y, m) = if self.month <= 2 { (self.year as i64 - 1, self.month as i64 + 9) } else { (self.year as i64, self.month as i64 - 3) };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    pub fn day_label(&self, today: &Stamp) -> String {
        const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
        const DAYS: [&str; 7] = ["thursday", "friday", "saturday", "sunday", "monday", "tuesday", "wednesday"];
        match today.days() - self.days() {
            0 => "today".into(),
            1 => "yesterday".into(),
            _ => format!("{} {} {}", DAYS[self.days().rem_euclid(7) as usize], self.day, MONTHS[(self.month as usize).saturating_sub(1) % 12]),
        }
    }
}

#[cfg(windows)]
pub fn now_local() -> Stamp {
    let t = unsafe { ::windows::Win32::System::SystemInformation::GetLocalTime() };
    Stamp { year: t.wYear as i32, month: t.wMonth as u32, day: t.wDay as u32, hour: t.wHour as u32, minute: t.wMinute as u32 }
}

#[cfg(not(windows))]
pub fn now_local() -> Stamp {
    // UTC: good enough until a time-zone source is wired in
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32;
    Stamp { year: y, month: m, day: d, hour: (rem / 3600) as u32, minute: ((rem % 3600) / 60) as u32 }
}

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
