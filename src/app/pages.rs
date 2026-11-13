//! Command palette, internal pages (history, settings), session restore,
//! and ephemeral windows.

use super::popups::Tone;
use super::*;
use crate::platform::{PopupKind, Stamp};
use crate::{HistRow, PaletteRow, PaletteWindow, SetRow};

/// One visited page. History lives in memory for the session only.
pub(super) struct Visit {
    pub url: String,
    pub title: String,
    pub at: Stamp,
}

#[derive(Clone, Copy)]
pub(super) enum PalTarget {
    Cmd(Command),
    Tab(u64),
}

pub(super) struct Palette {
    pub win: PaletteWindow,
    pub owned: bool,
    pub open: bool,
    pub opened_at: std::time::Instant,
    pub targets: Vec<PalTarget>,
}

const SECTIONS: [&str; 5] = ["appearance", "search & start", "shield", "privacy", "keys"];

fn seg(key: &str, label: &str, options: &[&str], value: &str, desc: &str) -> SetRow {
    SetRow {
        kind: 1,
        key: key.into(),
        label: label.into(),
        options: ModelRc::new(VecModel::from(options.iter().map(|o| SharedString::from(*o)).collect::<Vec<_>>())),
        value: value.into(),
        desc: desc.into(),
    }
}
fn text(kind: i32, key: &str, label: &str, value: &str, desc: &str) -> SetRow {
    SetRow { kind, key: key.into(), label: label.into(), options: ModelRc::default(), value: value.into(), desc: desc.into() }
}
fn heading(label: &str) -> SetRow {
    text(0, "", label, "", "")
}

const ENGINES: [(&str, &str); 3] = [
    ("duckduckgo", "https://duckduckgo.com/?q=%s"),
    ("startpage", "https://www.startpage.com/do/search?q=%s"),
    ("brave", "https://search.brave.com/search?q=%s"),
];

impl App {
    // -------------------------------------------------------------- palette

    pub(super) fn open_palette(&mut self) {
        self.close_menu();
        self.hide_card();
        let p = &self.palette.win;
        p.set_query(SharedString::new());
        paint(&p.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);
        self.fill_palette("");
        let win_w = self.ui.window().size().width as f32 / self.ui.window().scale_factor();
        let x = ((win_w - 600.0) / 2.0).max(0.0);
        let pos = self.to_screen(x, 44.0);
        let p = &self.palette.win;
        p.window().set_position(pos);
        let _ = p.show();
        if let (Some(popup), Some(owner)) = (hwnd_of(p.window()), self.hwnd()) {
            if !self.palette.owned {
                platform::own_popup(popup, owner, PopupKind::Menu, || post(Msg::PaletteClose));
                self.palette.owned = true;
            }
            platform::activate(popup);
        }
        p.invoke_take_keys();
        self.palette.open = true;
        self.palette.opened_at = std::time::Instant::now();
    }

    pub(super) fn fill_palette(&mut self, q: &str) {
        let mut rows = Vec::new();
        self.palette.targets.clear();
        for c in commands::table().into_iter().filter(|c| commands::matches(c.label, q)) {
            rows.push(PaletteRow { tag: "cmd".into(), label: c.label.into(), key: c.key.into() });
            self.palette.targets.push(PalTarget::Cmd(c.cmd));
        }
        for (i, t) in self.tabs.iter().enumerate() {
            let label = format!("tab {} · {}", i + 1, if t.title.is_empty() { "empty" } else { &t.title });
            if commands::matches(&label, q) || commands::matches(&t.url, q) {
                let key = if i < 8 { format!("ctrl+{}", i + 1) } else { String::new() };
                rows.push(PaletteRow { tag: "tab".into(), label: label.into(), key: key.into() });
                self.palette.targets.push(PalTarget::Tab(t.id));
            }
        }
        rows.truncate(9);
        self.palette.targets.truncate(9);
        self.palette.win.set_rows(ModelRc::new(VecModel::from(rows)));
        self.palette.win.set_sel(0);
    }

    pub(super) fn palette_run(&mut self, i: usize) {
        let target = self.palette.targets.get(i).copied();
        self.close_palette();
        match target {
            Some(PalTarget::Cmd(c)) => self.run(c),
            Some(PalTarget::Tab(id)) => {
                if let Some(i) = self.tab_index(id) {
                    self.activate(i);
                }
            }
            None => {}
        }
    }

    pub(super) fn close_palette(&mut self) {
        if self.palette.open {
            let _ = self.palette.win.hide();
            self.palette.open = false;
        }
    }

    // ------------------------------------------------------- internal pages

    pub(super) fn open_internal(&mut self, page: Internal) {
        // reuse the active tab if it is empty or already an internal page
        let t = &self.tabs[self.active];
        if t.wv.is_some() || (!t.url.is_empty() && t.page.is_none()) {
            self.new_tab_at(self.active + 1, None);
        }
        let t = &mut self.tabs[self.active];
        t.page = Some(page);
        t.url = page.url().to_string();
        t.title = match page {
            Internal::History => "history".into(),
            Internal::Settings => "settings".into(),
        };
        self.ui.invoke_blur_address();
        match page {
            Internal::History => {
                self.ui.set_hist_query(SharedString::new());
                self.fill_history("");
            }
            Internal::Settings => self.fill_settings(),
        }
    }

    pub(super) fn fill_history(&mut self, q: &str) {
        let today = platform::now_local();
        let mut rows = Vec::new();
        self.hist_index.clear();
        let mut last_day = String::new();
        for (i, v) in self.history.iter().enumerate().rev() {
            if !(commands::matches(&v.title, q) || commands::matches(&v.url, q)) {
                continue;
            }
            let day = v.at.day_label(&today);
            if day != last_day {
                rows.push(HistRow { kind: 0, time: SharedString::new(), title: day.clone().into(), url: SharedString::new() });
                self.hist_index.push(usize::MAX);
                last_day = day;
            }
            let url = v.url.trim_start_matches("https://").trim_start_matches("http://");
            rows.push(HistRow {
                kind: 1,
                time: format!("{:02}:{:02}", v.at.hour, v.at.minute).into(),
                title: if v.title.is_empty() { url.into() } else { v.title.as_str().into() },
                url: url.into(),
            });
            self.hist_index.push(i);
        }
        self.ui.set_hist_rows(ModelRc::new(VecModel::from(rows)));
        self.ui.set_hist_note(
            if self.cfg.ephemeral {
                "ephemeral window · nothing is kept"
            } else {
                "kept in memory for this session only · never written to disk"
            }
            .into(),
        );
    }

    pub(super) fn hist_open(&mut self, row: usize) {
        if let Some(&i) = self.hist_index.get(row) {
            if let Some(v) = self.history.get(i) {
                let url = v.url.clone();
                self.tabs[self.active].page = None;
                self.navigate_active(url);
            }
        }
    }

    pub(super) fn hist_forget(&mut self, row: usize) {
        if let Some(&i) = self.hist_index.get(row) {
            if i < self.history.len() {
                self.history.remove(i);
            }
        }
        let q = self.ui.get_hist_query().to_string();
        self.fill_history(&q);
    }

    pub(super) fn hist_clear(&mut self) {
        let n = self.history.len();
        self.history.clear();
        self.fill_history("");
        self.toast(format!("cleared {n} entr{}", if n == 1 { "y" } else { "ies" }), None, Tone::Ok);
    }

    // ------------------------------------------------------------- settings

    pub(super) fn fill_settings(&mut self) {
        let c = &self.cfg;
        let engine = ENGINES.iter().find(|e| e.1 == c.search).map_or("custom", |e| e.0);
        let start = match c.start.as_str() {
            "session" => "last session",
            "home" => "home",
            _ => "empty tab",
        };
        let filters_path = crate::config::dir().join("filters.txt");
        let filters = std::fs::read_to_string(&filters_path).unwrap_or_default();
        let n_rules = shield::Rules::builtin().load_str(&filters);
        let rows: Vec<SetRow> = match self.settings_section {
            0 => vec![
                heading("APPEARANCE"),
                text(4, "theme", "theme", "", "alt+t cycles themes from anywhere"),
                text(2, "font", "ui font", &c.font, "a monospace family installed on this machine"),
                seg("scrollbars", "page scrollbars", &["slim", "hidden", "system"], &c.scrollbars,
                    "slim draws a thin square bar in the theme's greys · hidden keeps scrolling but shows no bar"),
                seg("decorations", "window controls", &["auto", "always", "never"], &c.decorations,
                    "auto shows them on Windows and hides them under tiling compositors (Hyprland, Sway)"),
            ],
            1 => vec![
                heading("SEARCH & START"),
                seg("engine", "search engine", &["duckduckgo", "startpage", "brave", "custom"], engine, ""),
                text(2, "search", "search url", &c.search, "%s is replaced by what you type"),
                seg("start", "on start", &["empty tab", "last session", "home"], start, "last session reopens the tabs you had open"),
                text(2, "home", "home page", &c.home, "opened by on start: home, and by new tabs when set"),
                seg("tab_close", "closing a tab focuses", &["neighbour", "last used"], if c.tab_close == "last" { "last used" } else { "neighbour" }, ""),
            ],
            2 => vec![
                heading("SHIELD"),
                seg("shield", "shield", &["on", "off"], if self.shield.enabled() { "on" } else { "off" }, "off stops every layer until you turn it back on"),
                seg("beacons", "beacons to unknown hosts", &["scramble", "drop"], &c.beacons, "applies to tabs opened from now on"),
                seg("https", "upgrade to https", &["upgrade", "off"], &c.https, "addresses typed or followed as http:// are tried over https"),
                text(3, "", "built-in list", &format!("{} hosts and {} path rules", shield::lists::DOMAINS.len(), shield::lists::HOST_PATHS.len() + shield::lists::PATH_CONTAINS.len() + shield::lists::PATH_SUFFIX.len()), ""),
                text(5, "filters", "filters.txt", &filters, &format!("{n_rules} rules · saved when you leave the field · applied on next start")),
            ],
            3 => vec![
                heading("PRIVACY"),
                seg("clear_on_exit", "clear data on exit", &["off", "everything"], &c.clear_on_exit, "everything = cookies, cache and site storage"),
                seg("autoplay", "autoplay with sound", &["block", "allow"], &c.autoplay, "applies after restart"),
                text(3, "", "history", "kept in memory for this session only and never written to disk", ""),
            ],
            _ => {
                let mut v = vec![heading("KEYS")];
                for n in commands::table().into_iter().filter(|n| !n.key.is_empty()) {
                    v.push(text(3, "", n.key, n.label, ""));
                }
                v.push(text(3, "", "", "rebinding keys is not available yet", ""));
                v
            }
        };
        let ui = &self.ui;
        ui.set_set_sections(ModelRc::new(VecModel::from(SECTIONS.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>())));
        ui.set_set_section(self.settings_section as i32);
        ui.set_set_rows(ModelRc::new(VecModel::from(rows)));
        ui.set_theme_names(ModelRc::new(VecModel::from(THEMES.iter().map(|t| SharedString::from(t.short)).collect::<Vec<_>>())));
        ui.set_theme_acc(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.acc)).collect::<Vec<_>>())));
        ui.set_theme_bg(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.bg0)).collect::<Vec<_>>())));
        ui.set_theme_mid(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.mid)).collect::<Vec<_>>())));
        ui.set_theme_index(self.theme as i32);
    }

    /// Every change applies at once; there is no save button.
    pub(super) fn set_value(&mut self, key: &str, v: &str) {
        let v = v.to_string();
        match key {
            "font" if !v.trim().is_empty() => self.cfg.font = v.trim().to_string(),
            "decorations" => self.cfg.decorations = v,
            "scrollbars" => {
                self.cfg.scrollbars = v;
                self.push_scroll();
            }
            "engine" => {
                if let Some(e) = ENGINES.iter().find(|e| e.0 == v) {
                    self.cfg.search = e.1.to_string();
                }
            }
            "search" if v.contains("%s") => self.cfg.search = v,
            "search" => self.toast("the search url needs %s where the query goes", None, Tone::Warn),
            "start" => {
                self.cfg.start = match v.as_str() {
                    "last session" => "session",
                    "home" => "home",
                    _ => "empty",
                }
                .into()
            }
            "home" => self.cfg.home = v.trim().to_string(),
            "tab_close" => self.cfg.tab_close = if v == "last used" { "last".into() } else { "neighbour".into() },
            "shield" => {
                let on = v == "on";
                self.shield.set_enabled(on);
                self.cfg.shield = on;
                #[cfg(feature = "engine-cef")]
                engine::cef::set_shield(on);
            }
            "beacons" => {
                self.cfg.beacons = v;
                self.push_scroll();
            }
            "https" => {
                self.cfg.https = v;
                self.https.set(self.cfg.https == "upgrade");
                #[cfg(feature = "engine-cef")]
                engine::cef::set_https(self.cfg.https == "upgrade");
            }
            "filters" => {
                let path = crate::config::dir().join("filters.txt");
                if std::fs::read_to_string(&path).unwrap_or_default() != v {
                    let _ = std::fs::write(&path, &v);
                    self.toast("filters saved · applied on next start", None, Tone::Ok);
                }
            }
            "clear_on_exit" => self.cfg.clear_on_exit = v,
            "autoplay" => self.cfg.autoplay = v,
            _ => return,
        }
        self.cfg.save();
        self.apply_theme();
        self.apply_decorations();
        self.fill_settings();
    }

    pub(super) fn apply_decorations(&self) {
        let custom = platform::CUSTOM_TITLEBAR && self.titlebar_ok;
        self.ui.set_controls(custom && self.cfg.decorations != "never");
    }

    // -------------------------------------------------- session & ephemeral

    pub(super) fn session_path() -> std::path::PathBuf {
        crate::config::dir().join("session.json")
    }

    /// Called on quit: remember open tabs if the user asked for it, and mark
    /// browsing data for erasure.
    pub fn shutdown(&mut self) {
        if self.cfg.ephemeral {
            return;
        }
        if self.cfg.start == "session" {
            let urls: Vec<&str> = self.tabs.iter().filter(|t| t.page.is_none() && t.url.starts_with("http")).map(|t| t.url.as_str()).collect();
            let _ = std::fs::write(Self::session_path(), serde_json::to_string(&urls).unwrap_or_default());
        } else {
            let _ = std::fs::remove_file(Self::session_path());
        }
        if self.cfg.clear_on_exit == "everything" {
            // the engine still holds its files open; erase them on next start
            let _ = std::fs::write(crate::config::dir().join("wipe"), "");
        }
    }

    pub(super) fn spawn_ephemeral(&mut self) {
        match std::env::current_exe() {
            Ok(exe) => {
                if std::process::Command::new(exe).arg("--ephemeral").spawn().is_err() {
                    self.toast("could not open an ephemeral window", None, Tone::Bad);
                }
            }
            Err(_) => self.toast("could not open an ephemeral window", None, Tone::Bad),
        }
    }
}
