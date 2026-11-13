//! Features that talk to the page engine: find in page, downloads,
//! permission requests, site info, page menus, link status, error pages,
//! zoom, fullscreen and the per-site shield pause.

use super::popups::Tone;
use super::*;
use crate::platform::PopupKind;
use crate::{DlRow, PermRow, SiteInfoWindow, StatusWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlState {
    Running,
    Done,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub struct DlInfo {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub received: i64,
    pub total: i64,
    pub speed: i64,
    pub state: DlState,
}

/// Where a right-click happened and on what.
#[derive(Debug, Clone)]
pub struct MenuCtx {
    pub x: i32,
    pub y: i32,
    pub link: String,
    pub selection: String,
    pub editable: bool,
    pub page: String,
}

#[derive(Clone, Debug)]
pub(super) enum PageOp {
    OpenTab(String),
    OpenEphemeral(String),
    Copy(String),
    Back,
    Forward,
    Reload,
    Find(String),
    Search(String),
    Edit(&'static str),
    Inspect,
}

pub(super) struct PermReq {
    pub tab: u64,
    pub id: u64,
    pub host: String,
    pub kinds: u32,
}

pub(super) struct Features {
    pub find_case: bool,
    pub find_count: (i32, i32),
    pub downloads: Vec<DlInfo>,
    pub paused_dl: Vec<u32>,
    pub dl_open: bool,
    pub dl_done_until: Option<std::time::Instant>,
    pub perms: VecDeque<PermReq>,
    pub site: SiteInfoWindow,
    pub site_owned: bool,
    pub site_open: bool,
    pub site_opened_at: std::time::Instant,
    pub status: StatusWindow,
    pub status_owned: bool,
    pub status_visible: bool,
    pub zoom: HashMap<String, u32>,
    pub fullscreen: bool,
    pub reveal: bool,
    pub fs_hint_shown: bool,
    pub fs_timer: slint::Timer,
}

impl Features {
    pub fn new() -> Features {
        let site = SiteInfoWindow::new().expect("site info window");
        site.on_set(|k, v| post(Msg::SiteSet(k.into(), v.into())));
        site.on_clear_data(|| post(Msg::SiteClear));
        site.on_details(|| post(Msg::SiteDetails));
        site.on_dismiss(|| post(Msg::SiteClose));
        Features {
            find_case: false,
            find_count: (0, 0),
            downloads: Vec::new(),
            paused_dl: Vec::new(),
            dl_open: false,
            dl_done_until: None,
            perms: VecDeque::new(),
            site,
            site_owned: false,
            site_open: false,
            site_opened_at: std::time::Instant::now(),
            status: StatusWindow::new().expect("status window"),
            status_owned: false,
            status_visible: false,
            zoom: HashMap::new(),
            fullscreen: false,
            reveal: false,
            fs_hint_shown: false,
            fs_timer: slint::Timer::default(),
        }
    }
}

const PERM_CAMERA: u32 = 4;
const PERM_GEO: u32 = 256;
const PERM_MIC: u32 = 4096;
const ZOOM_STEPS: [u32; 13] = [30, 50, 67, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300];

fn kind_names(kinds: u32) -> Vec<&'static str> {
    let mut v = Vec::new();
    if kinds & PERM_CAMERA != 0 {
        v.push("camera");
    }
    if kinds & PERM_MIC != 0 {
        v.push("microphone");
    }
    if kinds & PERM_GEO != 0 {
        v.push("location");
    }
    v
}

fn mb(b: i64) -> String {
    format!("{:.1} MB", b.max(0) as f64 / 1_048_576.0)
}

pub(super) fn host_of(url: &str) -> String {
    urlparts::split(url).map_or(String::new(), |p| p.host.trim_start_matches("www.").to_string())
}

impl App {
    // ------------------------------------------------------------- find

    pub(super) fn open_find(&mut self) {
        if self.tabs[self.active].wv.is_none() {
            return;
        }
        self.ui.set_find_open(true);
        if let Some(wv) = &self.tabs[self.active].wv {
            wv.focus_parent();
        }
        self.ui.invoke_focus_find();
    }

    pub(super) fn find(&mut self, step: i32) {
        let text = self.ui.get_find_text().to_string();
        if let Some(wv) = &self.tabs[self.active].wv {
            wv.find(&text, step >= 0, self.feat.find_case, step != 0);
        }
        if text.is_empty() {
            self.feat.find_count = (0, 0);
        }
        self.show_find_count();
    }

    pub(super) fn close_find(&mut self) {
        self.ui.set_find_open(false);
        self.ui.set_find_text(SharedString::new());
        self.feat.find_count = (0, 0);
        if let Some(wv) = &self.tabs[self.active].wv {
            wv.stop_find(true);
            wv.focus();
        }
    }

    pub(super) fn on_find_result(&mut self, tab: u64, count: i32, active: i32) {
        if self.tabs.get(self.active).map(|t| t.id) == Some(tab) {
            self.feat.find_count = (count, active);
            self.show_find_count();
        }
    }

    fn show_find_count(&self) {
        let (count, active) = self.feat.find_count;
        let has_text = !self.ui.get_find_text().is_empty();
        self.ui.set_find_count(if !has_text { "".into() } else if count == 0 { "no match".into() } else { format!("{}/{count}", active.max(1)).into() });
        self.ui.set_find_none(has_text && count == 0);
        self.ui.set_find_case(self.feat.find_case);
    }

    // -------------------------------------------------------- downloads

    pub(super) fn on_download(&mut self, d: DlInfo) {
        let finished_now = d.state == DlState::Done
            && self.feat.downloads.iter().any(|x| x.id == d.id && x.state == DlState::Running);
        match self.feat.downloads.iter_mut().find(|x| x.id == d.id) {
            Some(x) => *x = d,
            None => self.feat.downloads.insert(0, d),
        }
        if finished_now {
            self.feat.dl_done_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
            slint::Timer::single_shot(std::time::Duration::from_millis(4100), || post(Msg::Tick));
        }
        self.refresh_downloads();
    }

    pub(super) fn refresh_downloads(&self) {
        let f = &self.feat;
        let running: Vec<&DlInfo> = f.downloads.iter().filter(|d| d.state == DlState::Running).collect();
        let cell = if !running.is_empty() {
            let (rec, tot) = running.iter().fold((0i64, 0i64), |a, d| (a.0 + d.received, a.1 + d.total.max(0)));
            if tot > 0 { format!("dl {}%", rec * 100 / tot) } else { "dl …".into() }
        } else if f.dl_done_until.is_some_and(|t| std::time::Instant::now() < t) {
            "dl done".into()
        } else if f.dl_open {
            "dl".into()
        } else {
            String::new()
        };
        self.ui.set_dl_done(running.is_empty() && cell == "dl done");
        self.ui.set_dl_cell(cell.into());
        self.ui.set_dl_open(f.dl_open);
        let rows: Vec<DlRow> = f
            .downloads
            .iter()
            .map(|d| {
                let paused = f.paused_dl.contains(&d.id);
                let (status, state, a1, a2) = match d.state {
                    DlState::Running => {
                        let speed = if paused { "paused".to_string() } else { format!("{}/s", mb(d.speed)) };
                        let size = if d.total > 0 { format!("{} / {}", mb(d.received), mb(d.total)) } else { mb(d.received) };
                        (format!("{size} · {speed}"), 0, if paused { "resume" } else { "pause" }, "cancel")
                    }
                    DlState::Done => (format!("{} · done", mb(d.received)), 1, "open", "show in folder"),
                    DlState::Failed => ("failed · the transfer stopped".into(), 2, "remove", ""),
                    DlState::Canceled => ("canceled".into(), 2, "remove", ""),
                };
                let pct = if d.state == DlState::Done { 1.0 } else if d.total > 0 { d.received as f32 / d.total as f32 } else { 0.0 };
                DlRow { name: d.name.as_str().into(), status: status.into(), pct, state, act1: a1.into(), act2: a2.into() }
            })
            .collect();
        self.ui.set_downloads(ModelRc::new(VecModel::from(rows)));
    }

    pub(super) fn dl_act(&mut self, row: usize, act: &str) {
        let Some(d) = self.feat.downloads.get(row).cloned() else { return };
        match act {
            "pause" | "resume" | "cancel" => {
                #[cfg(feature = "engine-cef")]
                engine::cef::download_action(d.id, match act { "pause" => "pause", "resume" => "resume", _ => "cancel" });
                if act == "pause" {
                    self.feat.paused_dl.push(d.id);
                } else {
                    self.feat.paused_dl.retain(|x| *x != d.id);
                }
            }
            "open" => {
                let _ = std::process::Command::new("explorer").arg(&d.path).spawn();
            }
            "show in folder" => {
                let _ = std::process::Command::new("explorer").arg(format!("/select,{}", d.path)).spawn();
            }
            "remove" => self.feat.downloads.retain(|x| x.id != d.id),
            _ => {}
        }
        self.refresh_downloads();
    }

    // ------------------------------------------------------ permissions

    fn perm_key(host: &str, kind: &str) -> String {
        format!("{host}|{kind}")
    }

    pub(super) fn on_perm_ask(&mut self, tab: u64, id: u64, origin: String, kinds: u32) {
        let host = host_of(&origin);
        let names = kind_names(kinds);
        let saved: Vec<Option<&String>> = names.iter().map(|k| self.cfg.perms.get(&Self::perm_key(&host, k))).collect();
        // remembered answers apply without asking
        if saved.iter().any(|v| v.is_some_and(|v| v == "block")) {
            #[cfg(feature = "engine-cef")]
            engine::cef::answer_prompt(id, false);
            return;
        }
        if !saved.is_empty() && saved.iter().all(|v| v.is_some_and(|v| v == "allow")) {
            #[cfg(feature = "engine-cef")]
            engine::cef::answer_prompt(id, true);
            return;
        }
        self.feat.perms.push_back(PermReq { tab, id, host, kinds });
        self.show_perm();
    }

    fn show_perm(&self) {
        let text = self.feat.perms.front().map_or(String::new(), |p| {
            let what = kind_names(p.kinds).join(" and ");
            format!("{} wants to use your {what}", p.host)
        });
        self.ui.set_perm_text(text.into());
    }

    pub(super) fn perm_answer(&mut self, answer: &str) {
        let Some(p) = self.feat.perms.pop_front() else { return };
        let allow = answer != "block";
        #[cfg(feature = "engine-cef")]
        engine::cef::answer_prompt(p.id, allow);
        if answer != "allow once" {
            for k in kind_names(p.kinds) {
                self.cfg.perms.insert(Self::perm_key(&p.host, k), if allow { "allow".into() } else { "block".into() });
            }
            self.cfg.save();
        }
        let what = kind_names(p.kinds).join(" and ");
        let msg = match answer {
            "block" => format!("{what} blocked for {}", p.host),
            "allow once" => format!("{what} allowed this time"),
            _ => format!("{what} allowed for {} · change it in site info", p.host),
        };
        self.toast(msg, None, if allow { Tone::Ok } else { Tone::Bad });
        let _ = p.tab;
        self.show_perm();
    }

    pub(super) fn on_perm_quiet(&mut self, tab: u64, _origin: String) {
        if let Some(i) = self.tab_index(tab) {
            self.tabs[i].bell = true;
        }
    }

    // ---------------------------------------------------------- site info

    pub(super) fn open_site_info(&mut self) {
        let t = &self.tabs[self.active];
        if t.url.is_empty() || t.page.is_some() {
            return;
        }
        let host = host_of(&t.url);
        let w = &self.feat.site;
        let (chip, tone, conn) = if t.cert_error {
            ("not private", 3, "the certificate did not check out · this page was not loaded")
        } else if t.url.starts_with("https://") {
            ("secure", 1, "encrypted with tls · the certificate checked out")
        } else {
            ("not encrypted", 2, "plain http · anyone on the network can read or change this page")
        };
        w.set_host(host.as_str().into());
        w.set_chip(chip.into());
        w.set_chip_tone(tone);
        w.set_conn(conn.into());
        w.set_paused(self.cfg.paused.iter().any(|h| *h == host));
        w.set_blocked_line(format!("{} blocked on this page", self.ledger.tab_total(t.id)).into());
        let rows: Vec<PermRow> = [("camera", "camera"), ("microphone", "microphone"), ("location", "location"), ("notifications", "notifications")]
            .iter()
            .map(|(label, key)| {
                let default = if *key == "notifications" { "block" } else { "ask" };
                let v = self.cfg.perms.get(&Self::perm_key(&host, key)).cloned().unwrap_or_else(|| default.into());
                PermRow { label: (*label).into(), key: (*key).into(), value: v.into() }
            })
            .collect();
        w.set_perms(ModelRc::new(VecModel::from(rows)));
        w.set_storage_line("cookies and site storage".into());
        paint(&w.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);
        let x = (self.ui.get_strip_x() + self.ui.get_strip_w() + 34.0 + 96.0).min(self.ui.get_content_w() - 382.0).max(0.0);
        w.window().set_position(self.to_screen(x, 32.0));
        let _ = w.show();
        if let (Some(p), Some(o)) = (hwnd_of(w.window()), self.hwnd()) {
            if !self.feat.site_owned {
                platform::own_popup(p, o, PopupKind::Menu, || post(Msg::SiteClose));
                self.feat.site_owned = true;
            }
            platform::activate(p);
        }
        w.invoke_take_keys();
        self.feat.site_open = true;
        self.feat.site_opened_at = std::time::Instant::now();
    }

    pub(super) fn close_site_info(&mut self) {
        if self.feat.site_open && self.feat.site_opened_at.elapsed().as_millis() > 80 {
            let _ = self.feat.site.hide();
            self.feat.site_open = false;
        }
    }

    pub(super) fn site_set(&mut self, key: &str, value: &str) {
        let host = host_of(&self.tabs[self.active].url);
        if key == "shield" {
            self.set_paused(&host, value == "paused");
        } else {
            self.cfg.perms.insert(Self::perm_key(&host, key), value.to_string());
            self.cfg.save();
        }
        // re-open in place with the new values
        self.feat.site_opened_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
        self.open_site_info();
    }

    pub(super) fn set_paused(&mut self, host: &str, paused: bool) {
        self.cfg.paused.retain(|h| h != host);
        if paused {
            self.cfg.paused.push(host.to_string());
        }
        self.cfg.save();
        #[cfg(feature = "engine-cef")]
        engine::cef::set_paused(&self.cfg.paused);
        // the runtime layer is set per page: reload to apply
        if let Some(wv) = &self.tabs[self.active].wv {
            wv.reload();
        }
        self.toast(
            if paused { format!("shield paused on {host}") } else { format!("shield back on for {host}") },
            None,
            if paused { Tone::Warn } else { Tone::Ok },
        );
    }

    pub(super) fn site_clear(&mut self) {
        let url = self.tabs[self.active].url.clone();
        #[cfg(feature = "engine-cef")]
        engine::cef::clear_site(&url);
        self.toast(format!("cleared cookies for {}", host_of(&url)), None, Tone::Ok);
    }

    // ---------------------------------------------------------- page menu

    pub(super) fn open_page_menu(&mut self, tab: u64, ctx: MenuCtx) {
        if self.tabs.get(self.active).map(|t| t.id) != Some(tab) {
            return;
        }
        let mut rows: Vec<(u8, String, &'static str, bool, MenuAct)> = Vec::new();
        let item = |label: String, key: &'static str, op: PageOp| (0u8, label, key, true, MenuAct::Page(op));
        let sep = || (1u8, String::new(), "", true, MenuAct::Nothing);
        if !ctx.link.is_empty() {
            let clean = params::clean_navigation(&ctx.link);
            rows.push(item("open link in new tab".into(), "", PageOp::OpenTab(ctx.link.clone())));
            rows.push(item("open link in ephemeral window".into(), "", PageOp::OpenEphemeral(ctx.link.clone())));
            rows.push(sep());
            match clean {
                Some(c) => {
                    rows.push(item(format!("copy clean link  (strips {})", c.removed.len()), "", PageOp::Copy(c.url)));
                    rows.push(item("copy link as-is".into(), "", PageOp::Copy(ctx.link.clone())));
                }
                None => rows.push(item("copy link".into(), "", PageOp::Copy(ctx.link.clone()))),
            }
            rows.push(sep());
        } else if !ctx.selection.trim().is_empty() {
            let sel: String = ctx.selection.trim().chars().take(24).collect();
            rows.push(item("copy".into(), "ctrl+c", PageOp::Edit("copy")));
            rows.push(item(format!("search for \u{201c}{sel}\u{201d}"), "", PageOp::Search(ctx.selection.trim().to_string())));
            rows.push(item(format!("find \u{201c}{sel}\u{201d}"), "", PageOp::Find(ctx.selection.trim().to_string())));
            rows.push(sep());
        } else if ctx.editable {
            rows.push(item("cut".into(), "ctrl+x", PageOp::Edit("cut")));
            rows.push(item("copy".into(), "ctrl+c", PageOp::Edit("copy")));
            rows.push(item("paste".into(), "ctrl+v", PageOp::Edit("paste")));
            rows.push(item("select all".into(), "ctrl+a", PageOp::Edit("selectall")));
            rows.push(sep());
        } else {
            let (back, fwd) = self.tabs[self.active].wv.as_ref().map_or((false, false), |w| (w.can_go_back(), w.can_go_forward()));
            rows.push((0, "back".into(), "alt+left", back, MenuAct::Page(PageOp::Back)));
            rows.push((0, "forward".into(), "alt+right", fwd, MenuAct::Page(PageOp::Forward)));
            rows.push(item("reload".into(), "ctrl+r", PageOp::Reload));
            rows.push(sep());
            rows.push(item("find in page".into(), "ctrl+f", PageOp::Find(String::new())));
            let page = params::clean_navigation(&ctx.page).map_or(ctx.page.clone(), |c| c.url);
            rows.push(item("copy clean page link".into(), "", PageOp::Copy(page)));
            rows.push(item("view source".into(), "ctrl+u", PageOp::Edit("source")));
            rows.push(sep());
        }
        rows.push(item("inspect".into(), "ctrl+shift+i", PageOp::Inspect));
        let scale = self.ui.window().scale_factor();
        let x = ctx.x as f32 / scale.max(1.0);
        let y = self.ui.get_content_y() + ctx.y as f32 / scale.max(1.0);
        let win_h = self.ui.window().size().height as f32 / scale;
        let h: f32 = 12.0 + rows.iter().map(|r| if r.0 == 1 { 9.0 } else { 26.0 }).sum::<f32>();
        self.show_menu_rows(rows, x, y.min(win_h - h - 4.0).max(32.0), 290.0);
    }

    pub(super) fn page_op(&mut self, op: PageOp) {
        let wv = self.tabs[self.active].wv.as_ref();
        match op {
            PageOp::OpenTab(u) => {
                let at = self.active + 1;
                let keep = self.active;
                self.new_tab_at(at, Some(u));
                // a link opened from the menu goes to the background
                self.activate(keep);
                self.toast(format!("opened in tab {}", at + 1), None, Tone::Plain);
            }
            PageOp::OpenEphemeral(u) => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--ephemeral").arg(u).spawn();
                }
            }
            PageOp::Copy(u) => {
                if platform::set_clipboard(&u) {
                    self.toast("link copied", None, Tone::Ok);
                }
            }
            PageOp::Back => {
                if let Some(w) = wv {
                    w.go_back();
                }
            }
            PageOp::Forward => {
                if let Some(w) = wv {
                    w.go_forward();
                }
            }
            PageOp::Reload => {
                if let Some(w) = wv {
                    w.reload();
                }
            }
            PageOp::Find(text) => {
                self.ui.set_find_text(text.into());
                self.open_find();
                self.find(0);
            }
            PageOp::Search(q) => {
                let url = self.cfg.search.replace("%s", &urlparts::percent_encode(&q));
                self.new_tab_at(self.active + 1, Some(url));
            }
            PageOp::Edit(what) => {
                if let Some(w) = wv {
                    w.edit(what);
                }
            }
            PageOp::Inspect => {
                if let Some(w) = wv {
                    w.devtools();
                }
            }
        }
    }

    // -------------------------------------------------------- link status

    pub(super) fn on_status(&mut self, tab: u64, url: String) {
        let active = self.tabs.get(self.active).map(|t| t.id) == Some(tab);
        if !active || url.is_empty() || self.menu.open {
            if self.feat.status_visible {
                let _ = self.feat.status.hide();
                self.feat.status_visible = false;
            }
            return;
        }
        let s = &self.feat.status;
        match params::clean_navigation(&url) {
            Some(c) => {
                s.set_target(c.url.into());
                s.set_note(format!("strips {}", c.removed.join(", ")).into());
            }
            None => {
                s.set_target(url.into());
                s.set_note(SharedString::new());
            }
        }
        paint(&s.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);
        let y = self.ui.get_content_y() + self.ui.get_content_h() - 24.0;
        s.window().set_position(self.to_screen(0.0, y));
        let _ = s.show();
        if !self.feat.status_owned {
            if let (Some(p), Some(o)) = (hwnd_of(s.window()), self.hwnd()) {
                platform::own_popup(p, o, PopupKind::Card, || {});
                self.feat.status_owned = true;
            }
        }
        self.feat.status_visible = true;
    }

    // -------------------------------------------------------- error pages

    /// A flat internal page; buttons post commands back over window.ipc.
    pub(super) fn info_page(&self, chip: &str, tone: u32, title: &str, line: &str, note: &str, buttons: &[(&str, &str, bool)]) -> String {
        let p = &THEMES[self.theme];
        let mix = |c: u32| -> u32 {
            let m = |sh: u32| (((c >> sh) & 0xff) * 22 + ((p.bg0 >> sh) & 0xff) * 78) / 100;
            (m(16) << 16) | (m(8) << 8) | m(0)
        };
        let btns: String = buttons
            .iter()
            .map(|(label, cmd, primary)| {
                let style = if *primary {
                    format!("background:#{:06x};color:#{:06x};border:1px solid #{:06x};font-weight:700", p.acc, p.bg0, p.acc)
                } else {
                    format!("background:transparent;color:#{:06x};border:1px solid #{:06x}", p.fg, p.bg3)
                };
                format!(
                    "<button style=\"font:inherit;padding:3px 12px;cursor:pointer;{style}\" onclick=\"window.ipc&&ipc.postMessage(JSON.stringify({{cmd:'{cmd}'}}))\">{}</button>",
                    html_escape(label)
                )
            })
            .collect();
        format!(
            "<!doctype html><meta charset=utf-8><title>{t}</title>\
             <body style=\"margin:0;height:100vh;display:flex;align-items:flex-end;background:#{bg:06x};color:#{fg:06x};font:12px {font},monospace\">\
             <div style=\"width:100%;padding:16px 24px 24px;border-top:1px solid #{b3:06x};display:flex;flex-direction:column;gap:10px\">\
             <div><span style=\"color:#{tone:06x};background:#{tint:06x};padding:2px 6px\">{chip}</span></div>\
             <div style=\"font-weight:700\">{t}</div>\
             <div style=\"color:#{mid:06x}\">{line}</div>\
             <div style=\"color:#{mid:06x}\">{note}</div>\
             <div style=\"display:flex;gap:8px;margin-top:6px\">{btns}</div></div></body>",
            t = html_escape(title),
            bg = p.bg0,
            fg = p.fg,
            font = html_escape(&self.cfg.font),
            b3 = p.bg3,
            tone = tone,
            tint = mix(tone),
            chip = html_escape(chip),
            mid = p.mid,
            line = html_escape(line),
            note = note,
        )
    }

    pub(super) fn show_page(&mut self, tab: u64, html: String) {
        if let Some(i) = self.tab_index(tab) {
            self.tabs[i].loading = false;
            if let Some(wv) = &self.tabs[i].wv {
                wv.load_html(&html);
            }
        }
    }

    pub(super) fn on_load_error(&mut self, tab: u64, url: String, code: i32, text: String) {
        let Some(i) = self.tab_index(tab) else { return };
        self.tabs[i].url = url.clone();
        let host = host_of(&url);
        let p = &THEMES[self.theme];
        let html = self.info_page(
            "offline",
            p.warn,
            &format!("can't reach {host}"),
            &format!("{} · error {code}", if text.is_empty() { "the page did not load" } else { &text }),
            "brxswer retries when you press retry or reload.",
            &[("retry", "retry", true)],
        );
        self.show_page(tab, html);
    }

    pub(super) fn on_cert_error(&mut self, tab: u64, url: String, code: i32) {
        let Some(i) = self.tab_index(tab) else { return };
        self.tabs[i].url = url.clone();
        self.tabs[i].cert_error = true;
        let host = host_of(&url);
        let p = &THEMES[self.theme];
        let html = self.info_page(
            "not private",
            p.bad,
            &format!("connection not private · {host}"),
            &format!("the certificate did not check out (error {code}) · anyone on the network could read or change this page"),
            &format!("There is no proceed button. To go ahead once anyway, type the address with <span style=\"color:#{:06x}\">!</span> in front.", p.acc),
            &[("go back", "back", true)],
        );
        self.show_page(tab, html);
    }

    pub(super) fn crash_page(&mut self, tab: u64) {
        let p = &THEMES[self.theme];
        let n = self.tab_index(tab).map_or(0, |i| i + 1);
        let html = self.info_page(
            "crashed",
            p.bad,
            &format!("tab {n} stopped"),
            "the page process ended · nothing was lost in other tabs",
            "Reload brings the page back.",
            &[("reload", "revive", true), ("close tab", "closetab", false)],
        );
        self.show_page(tab, html);
    }

    /// Commands from internal pages' buttons.
    pub(super) fn page_command(&mut self, tab: u64, cmd: &str) {
        let Some(i) = self.tab_index(tab) else { return };
        let url = self.tabs[i].url.clone();
        match cmd {
            "back" => {
                if let Some(wv) = &self.tabs[i].wv {
                    wv.go_back();
                }
            }
            "retry" | "revive" => {
                self.tabs[i].crashed = false;
                self.tabs[i].cert_error = false;
                if let Some(wv) = &self.tabs[i].wv {
                    wv.load_url(&url);
                }
            }
            "allowsite" => {
                let host = host_of(&url);
                shield::allow_host(&host);
                let path = crate::config::dir().join("filters.txt");
                let mut f = std::fs::read_to_string(&path).unwrap_or_default();
                if !f.ends_with('\n') && !f.is_empty() {
                    f.push('\n');
                }
                f.push_str(&format!("@@||{host}^\n"));
                let _ = std::fs::write(&path, f);
                self.tabs[i].blocked = false;
                self.toast(format!("allowed {host} · added @@||{host}^ to filters.txt"), None, Tone::Ok);
                if let Some(wv) = &self.tabs[i].wv {
                    wv.load_url(&url);
                }
            }
            "closetab" => self.close_tab(i),
            _ => {}
        }
    }

    // --------------------------------------------------------------- zoom

    pub(super) fn zoom_step(&mut self, dir: i32) {
        let host = host_of(&self.tabs[self.active].url);
        let cur = self.feat.zoom.get(&host).copied().unwrap_or(100);
        let next = match dir {
            0 => 100,
            d if d > 0 => ZOOM_STEPS.iter().copied().find(|z| *z > cur).unwrap_or(300),
            _ => ZOOM_STEPS.iter().rev().copied().find(|z| *z < cur).unwrap_or(30),
        };
        if next == 100 {
            self.feat.zoom.remove(&host);
        } else {
            self.feat.zoom.insert(host, next);
        }
        if let Some(wv) = &self.tabs[self.active].wv {
            wv.set_zoom(next);
        }
        self.menu.win.set_zoom_pct(next as i32);
    }

    pub(super) fn current_zoom(&self) -> u32 {
        self.tabs.get(self.active).map_or(100, |t| self.feat.zoom.get(&host_of(&t.url)).copied().unwrap_or(100))
    }

    // --------------------------------------------------------- fullscreen

    pub(super) fn set_fullscreen(&mut self, on: bool) {
        if self.feat.fullscreen == on {
            return;
        }
        self.feat.fullscreen = on;
        self.feat.reveal = false;
        self.ui.window().set_fullscreen(on);
        self.ui.set_fullscreen(on);
        self.ui.set_reveal(false);
        if on {
            self.feat.fs_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), || post(Msg::RevealCheck));
            if !self.feat.fs_hint_shown {
                self.feat.fs_hint_shown = true;
                self.toast("F11 leaves fullscreen", None, Tone::Plain);
            }
        } else {
            self.feat.fs_timer.stop();
        }
    }

    /// While fullscreen: pointer at the top edge brings the bar back.
    pub(super) fn reveal_check(&mut self) {
        let Some(h) = self.hwnd() else { return };
        let Some(y) = platform::cursor_client_y(h) else { return };
        let scale = self.ui.window().scale_factor();
        let y = y as f32 / scale;
        let reveal = if self.feat.reveal { y < 64.0 } else { y < 4.0 };
        if reveal != self.feat.reveal {
            self.feat.reveal = reveal;
            self.ui.set_reveal(reveal);
        }
    }
}
