//! Application core: one message queue, one owner of all state.
//!
//! Every engine callback and UI callback only *posts* a `Msg`. The queue is
//! drained on the event loop, so no callback ever re-enters app state while
//! the engine is pumping messages.

use crate::config::Config;
use crate::omnibox;
use crate::platform::{self, ChromeEvent};
use crate::shield::{self, params, urlparts, Layer, Ledger, Shield, ShieldEvent, Verdict};
use crate::theme::{self, THEMES};
use crate::{AppWindow, CardWindow, PaletteWindow, LayerRow, LogRow, MenuRow, MenuWindow, Suggestion, TabItem, Theme, ToastWindow};
use std::collections::VecDeque as Deque;

mod commands;
mod pages;
mod popups;
mod strip;
use pages::{Palette, Visit};
use popups::{Card, Toast, ToastAct, Tone};
use strip::TabOp;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use wry::dpi::{LogicalPosition, LogicalSize};
use crate::engine::{self, Kind, Page};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebViewBuilder};

#[cfg(windows)]
use wry::WebViewBuilderExtWindows;

// ------------------------------------------------------------------ commands

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Address,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    Jump(usize),
    Back,
    Forward,
    Reload,
    Panel,
    Theme,
    Quit,
    Reopen,
    Mute,
    LastTab,
    Palette,
    SetTheme(usize),
    History,
    Settings,
    Ephemeral,
    CopyLink,
}

impl Command {
    pub fn parse(combo: &str) -> Option<Command> {
        Some(match combo {
            "ctrl+l" => Command::Address,
            "ctrl+t" => Command::NewTab,
            "ctrl+w" => Command::CloseTab,
            "ctrl+Tab" => Command::NextTab,
            "ctrl+shift+Tab" => Command::PrevTab,
            "ctrl+b" => Command::Panel,
            "ctrl+r" | "F5" => Command::Reload,
            "alt+ArrowLeft" => Command::Back,
            "alt+ArrowRight" => Command::Forward,
            "alt+t" => Command::Theme,
            "ctrl+q" => Command::Quit,
            "ctrl+shift+t" => Command::Reopen,
            "ctrl+m" => Command::Mute,
            "ctrl+9" => Command::LastTab,
            "ctrl+k" => Command::Palette,
            "ctrl+h" => Command::History,
            "ctrl+," => Command::Settings,
            "ctrl+shift+n" => Command::Ephemeral,
            _ => {
                let d = combo.strip_prefix("ctrl+")?;
                let n: usize = d.parse().ok().filter(|n| (1..=9).contains(n))?;
                Command::Jump(n - 1)
            }
        })
    }
}

/// Builds the canonical combo string from a Slint key event.
pub fn combo_from_slint(text: &str, ctrl: bool, alt: bool, shift: bool) -> Option<String> {
    use slint::platform::Key;
    let c = text.chars().next()?;
    let key = if c == char::from(Key::Tab) || c == char::from(Key::Backtab) {
        "Tab".to_string()
    } else if c == char::from(Key::LeftArrow) {
        "ArrowLeft".to_string()
    } else if c == char::from(Key::RightArrow) {
        "ArrowRight".to_string()
    } else if c == char::from(Key::F5) {
        "F5".to_string()
    } else if (c as u32) < 0x20 && ctrl {
        // some platforms deliver ctrl+letter as a control character
        (((c as u8) + 0x60) as char).to_string()
    } else if c.is_ascii_alphanumeric() || c == ',' {
        c.to_ascii_lowercase().to_string()
    } else {
        return None;
    };
    let shift = shift || c == char::from(Key::Backtab);
    let s = format!(
        "{}{}{}{}",
        if ctrl { "ctrl+" } else { "" },
        if alt { "alt+" } else { "" },
        if shift { "shift+" } else { "" },
        key
    );
    Command::parse(&s).map(|_| s)
}

// ------------------------------------------------------------------ messages

#[derive(Debug)]
pub enum Msg {
    Startup(Vec<String>),
    Key(String),
    Geometry,
    Submit(String),
    Edited(String),
    Complete(usize),
    TabClicked(usize),
    NewTab,
    Back,
    Forward,
    Reload,
    TogglePanel,
    SetShield(bool),
    PageFocus,
    Title(u64, String),
    LoadStarted(u64, String),
    LoadFinished(u64, String),
    OpenTab(String),
    Redirect(u64, String),
    BlockedNav(u64, String, String),
    Ipc(u64, String),
    Shield(ShieldEvent),
    WebFocus,
    Chrome(ChromeEvent),
    WinMinimize,
    WinMaximize,
    WinClose,
    MenuToggle,
    MenuPick(usize),
    MenuTheme(usize),
    MenuClose,
    TabClose(usize),
    TabMute(usize),
    TabMenu(usize),
    TabHover(usize, bool),
    TabDrag(usize, f32),
    TabDrop(usize, f32),
    TabGeom(usize, f32, f32),
    StripStep(i32),
    StripWheel(f32),
    CardShow(u64),
    CardHide(u64),
    ToastAction,
    ToastHide(u64),
    Audio(u64, bool),
    Crashed(u64, bool),
    Tick,
    /// real loading progress, 0..1 (engines that report it)
    Progress(u64, f32),
    PaletteEdited(String),
    PaletteRun(usize),
    PaletteClose,
    HistSearch(String),
    HistOpen(usize),
    HistForget(usize),
    HistClear,
    SetSection(usize),
    SetValue(String, String),
    ReopenRecent(usize),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Msg>> = const { RefCell::new(VecDeque::new()) };
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn post(m: Msg) {
    QUEUE.with(|q| q.borrow_mut().push_back(m));
    if !SCHEDULED.replace(true) {
        let _ = slint::invoke_from_event_loop(drain);
    }
}

fn drain() {
    SCHEDULED.set(false);
    APP.with(|cell| {
        // The engine may pump the message loop inside one of our calls;
        // if we are already handling messages, try again on the next turn.
        let Ok(mut guard) = cell.try_borrow_mut() else {
            if !SCHEDULED.replace(true) {
                let _ = slint::invoke_from_event_loop(drain);
            }
            return;
        };
        let Some(app) = guard.as_mut() else { return };
        let mut n = 0;
        let debug = std::env::var_os("BRX_DEBUG").is_some();
        while let Some(m) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
            if debug && !matches!(m, Msg::Shield(_) | Msg::TabGeom(..)) {
                eprintln!("[msg] {m:?}");
            }
            app.handle(m);
            n += 1;
        }
        if n > 0 {
            app.refresh();
        }
    });
}

// ----------------------------------------------------------------------- app

struct Tab {
    id: u64,
    wv: Option<Page>,
    url: String,
    title: String,
    loading: bool,
    /// Parameter-strip redirects since the last finished load (loop guard).
    strips: Rc<Cell<u8>>,
    pinned: bool,
    audio: bool,
    muted: bool,
    crashed: bool,
    /// Put to sleep after being hidden for a while.
    trimmed: bool,
    hidden_since: Option<std::time::Instant>,
    /// The shield refused this page; the block page is showing.
    blocked: bool,
    /// Loading progress 0..1 when the engine reports it, else 0.
    progress: f32,
    /// An internal page (history, settings) shown instead of a web view.
    page: Option<Internal>,
}

/// Internal pages, drawn by Slint instead of the web engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    History,
    Settings,
}

impl Internal {
    fn url(self) -> &'static str {
        match self {
            Internal::History => "brx://history",
            Internal::Settings => "brx://settings",
        }
    }
}

impl Tab {
    fn new(id: u64) -> Tab {
        Tab {
            id,
            wv: None,
            url: String::new(),
            title: String::new(),
            loading: false,
            strips: Rc::new(Cell::new(0)),
            pinned: false,
            audio: false,
            muted: false,
            crashed: false,
            trimmed: false,
            hidden_since: None,
            blocked: false,
            progress: 0.0,
            page: None,
        }
    }
}

pub struct App {
    ui: AppWindow,
    cfg: Config,
    shield: Rc<Shield>,
    ledger: Ledger,
    script: String,
    ctx: WebContext,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    theme: usize,
    panel: bool,
    history: Vec<Visit>,
    /// history page row -> index into `history` (usize::MAX for day headers)
    hist_index: Vec<usize>,
    settings_section: usize,
    palette: Palette,
    /// shared with the navigation layer: upgrade http:// to https://
    https: Rc<Cell<bool>>,
    titlebar_ok: bool,
    narrow: bool,
    engine: Kind,
    trace: bool,
    /// tab ids, most recently used first (for "closing a tab focuses: last used")
    mru: Vec<u64>,
    sugg_targets: Vec<Target>,
    menu: Menu,
    card: Card,
    toast: Toast,
    /// Tab cell (x, width) inside the strip row, by tab index.
    geoms: Vec<(f32, f32)>,
    strip_scroll: f32,
    /// (dragged tab, drop index) while a tab is being dragged.
    drag: Option<(usize, usize)>,
    /// Recently closed (url, title), newest first.
    closed: Deque<(String, String)>,
    // kept alive for its 30 s tick
    _ticker: slint::Timer,
}

/// What picking an address-bar suggestion does.
#[derive(Clone, Debug)]
enum Target {
    Url(String),
    Tab(u64),
    Cmd(Command),
}

/// What a main-menu row does.
#[derive(Clone, Copy)]
enum MenuAct {
    Run(Command),
    Tab(TabOp, usize),
    Nothing,
}

struct Menu {
    win: MenuWindow,
    owned: bool,
    open: bool,
    opened_at: std::time::Instant,
    closed_at: Option<std::time::Instant>,
    acts: Vec<MenuAct>,
}

fn hwnd_of(w: &slint::Window) -> Option<isize> {
    use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = w.window_handle();
    let h = handle.window_handle().ok()?;
    match h.as_raw() {
        #[cfg(windows)]
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

fn paint(g: &Theme<'_>, p: &theme::Palette, font: &str) {
    g.set_bg0(theme::color(p.bg0));
    g.set_bg1(theme::color(p.bg1));
    g.set_bg2(theme::color(p.bg2));
    g.set_bg3(theme::color(p.bg3));
    g.set_fg(theme::color(p.fg));
    g.set_acc(theme::color(p.acc));
    g.set_mid(theme::color(p.mid));
    g.set_dim(theme::color(p.dim));
    g.set_ok(theme::color(p.ok));
    g.set_info(theme::color(p.info));
    g.set_warn(theme::color(p.warn));
    g.set_bad(theme::color(p.bad));
    g.set_name(p.short.into());
    g.set_font(font.into());
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl App {
    pub fn new(ui: AppWindow, cfg: Config, shield: Rc<Shield>) -> App {
        let script = shield::runtime_script(&shield.rules, &cfg.beacons);
        // ephemeral windows keep their engine data in a throwaway folder
        let data = if cfg.ephemeral {
            std::env::temp_dir().join(format!("brxswer-eph-{}", std::process::id()))
        } else {
            crate::config::dir().join("webdata")
        };
        let ctx = WebContext::new(Some(data));
        let pal_win = PaletteWindow::new().expect("palette window");
        pal_win.on_edited(|q| post(Msg::PaletteEdited(q.into())));
        pal_win.on_run(|i| post(Msg::PaletteRun(i.max(0) as usize)));
        pal_win.on_dismiss(|| post(Msg::PaletteClose));
        let https = Rc::new(Cell::new(cfg.https == "upgrade"));
        let theme = theme::index_of(&cfg.theme);
        let menu_win = MenuWindow::new().expect("menu window");
        menu_win.on_pick(|i| post(Msg::MenuPick(i.max(0) as usize)));
        menu_win.on_pick_theme(|i| post(Msg::MenuTheme(i.max(0) as usize)));
        menu_win.on_dismiss(|| post(Msg::MenuClose));
        let card_win = CardWindow::new().expect("card window");
        let toast_win = ToastWindow::new().expect("toast window");
        toast_win.on_act(|| post(Msg::ToastAction));
        let ticker = slint::Timer::default();
        ticker.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(30), || post(Msg::Tick));
        App {
            ui,
            cfg,
            shield,
            ledger: Ledger::default(),
            script,
            ctx,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            theme,
            panel: false,
            history: Vec::new(),
            hist_index: Vec::new(),
            settings_section: 0,
            palette: Palette { win: pal_win, owned: false, open: false, opened_at: std::time::Instant::now(), targets: Vec::new() },
            https,
            titlebar_ok: false,
            narrow: false,
            engine: Kind::WebView2,
            trace: std::env::var_os("BRX_TRACE").is_some(),
            mru: Vec::new(),
            sugg_targets: Vec::new(),
            menu: Menu {
                win: menu_win,
                owned: false,
                open: false,
                opened_at: std::time::Instant::now(),
                closed_at: None,
                acts: Vec::new(),
            },
            card: Card { win: card_win, owned: false, visible: false, seq: 0, tab: None },
            toast: Toast { win: toast_win, owned: false, seq: 0, act: None },
            geoms: Vec::new(),
            strip_scroll: 0.0,
            drag: None,
            closed: Deque::new(),
            _ticker: ticker,
        }
    }

    pub fn set_engine(&mut self, k: Kind) {
        self.engine = k;
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn content_rect(&self) -> Rect {
        Rect {
            position: LogicalPosition::new(0.0, self.ui.get_content_y() as f64).into(),
            size: LogicalSize::new(self.ui.get_content_w().max(1.0) as f64, self.ui.get_content_h().max(1.0) as f64)
                .into(),
        }
    }

    fn hwnd(&self) -> Option<isize> {
        hwnd_of(self.ui.window())
    }


    // ------------------------------------------------------------- webviews

    fn create_webview(&mut self, idx: usize, url: &str) -> bool {
        let id = self.tabs[idx].id;
        #[cfg(feature = "engine-cef")]
        if self.engine == Kind::Chromium && !engine::cef::ensure_started(self.cfg.ephemeral) {
            // engine files missing or broken: fall back to the system engine
            eprintln!("brxswer: bundled Chromium failed to start, using WebView2");
            self.engine = Kind::WebView2;
        }
        #[cfg(feature = "engine-cef")]
        if self.engine == Kind::Chromium {
            let Some(parent) = self.hwnd() else { return false };
            let r = self.content_rect();
            let scale = self.ui.window().scale_factor() as f64;
            let p = r.position.to_physical::<i32>(scale);
            let z = r.size.to_physical::<i32>(scale);
            let h = engine::cef::Handle::create(id, parent, (p.x, p.y, z.width, z.height), url, self.cfg.ephemeral);
            if self.tabs[idx].muted {
                h.set_muted(true);
            }
            self.tabs[idx].crashed = false;
            self.tabs[idx].wv = Some(Page::Cef(h));
            return true;
        }
        let strips = self.tabs[idx].strips.clone();
        let nav_shield = self.shield.clone();
        let https = self.https.clone();
        let rect = self.content_rect();
        let p = &THEMES[self.theme];
        let handle = self.ui.window().window_handle();
        let builder = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_bounds(rect)
            .with_url(url)
            .with_initialization_script_for_main_only(self.script.clone(), false)
            .with_ipc_handler(move |req| post(Msg::Ipc(id, req.into_body())))
            .with_navigation_handler(move |url| navigation_layer(id, &nav_shield, &strips, https.get(), &url))
            .with_incognito(self.cfg.ephemeral)
            .with_autoplay(self.cfg.autoplay == "allow")
            .with_new_window_req_handler(move |url, _| {
                post(Msg::OpenTab(url));
                NewWindowResponse::Deny
            })
            .with_document_title_changed_handler(move |t| post(Msg::Title(id, t)))
            .with_on_page_load_handler(move |ev, url| {
                post(match ev {
                    PageLoadEvent::Started => Msg::LoadStarted(id, url),
                    PageLoadEvent::Finished => Msg::LoadFinished(id, url),
                })
            })
            .with_focused(true);
        #[cfg(windows)]
        let builder = builder
            .with_additional_browser_args(if self.cfg.autoplay == "allow" {
                format!("{} --autoplay-policy=no-user-gesture-required", platform::BROWSER_ARGS)
            } else {
                format!("{} --autoplay-policy=document-user-activation-required", platform::BROWSER_ARGS)
            })
            .with_theme(if p.dark { wry::Theme::Dark } else { wry::Theme::Light });
        let wv = match builder.build_as_child(&handle) {
            Ok(wv) => wv,
            Err(e) => {
                eprintln!("brxswer: webview creation failed: {e}");
                return false;
            }
        };
        if let Err(e) = platform::install_transport(&wv, id, self.shield.clone()) {
            eprintln!("brxswer: transport layer unavailable: {e:?}");
        }
        let _ = platform::install_accelerators(&wv, |c| post(Msg::Key(c)));
        let _ = platform::install_focus(&wv, || post(Msg::WebFocus));
        let _ = platform::install_media(&wv, move |on| post(Msg::Audio(id, on)), move |all| post(Msg::Crashed(id, all)));
        if self.tabs[idx].muted {
            platform::set_muted(&wv, true);
        }
        self.tabs[idx].crashed = false;
        self.tabs[idx].wv = Some(Page::Wry(wv));
        true
    }

    fn navigate_active(&mut self, url: String) {
        let idx = self.active;
        self.tabs[idx].page = None;
        self.tabs[idx].url = url.clone();
        self.tabs[idx].loading = true;
        match &self.tabs[idx].wv {
            Some(wv) => {
                wv.load_url(&url);
                wv.set_visible(true);
                wv.focus();
            }
            None => {
                self.create_webview(idx, &url);
            }
        }
        self.ui.invoke_blur_address();
        self.layout();
    }

    fn show_active(&mut self) {
        let now = std::time::Instant::now();
        if let Some(t) = self.tabs.get(self.active) {
            let id = t.id;
            self.mru.retain(|x| *x != id);
            self.mru.insert(0, id);
        }
        for (i, t) in self.tabs.iter_mut().enumerate() {
            let on = i == self.active;
            if on {
                t.hidden_since = None;
                t.trimmed = false;
            } else if t.hidden_since.is_none() {
                t.hidden_since = Some(now);
            }
            if let Some(wv) = &t.wv {
                wv.set_visible(on);
                wv.set_background(!on);
            }
        }
        self.layout();
        match &self.tabs[self.active].wv {
            Some(wv) => {
                self.ui.invoke_blur_address();
                wv.focus();
            }
            None => self.focus_address(),
        }
    }

    fn layout(&self) {
        // geometry events can arrive before the first tab exists
        if let Some(wv) = self.tabs.get(self.active).and_then(|t| t.wv.as_ref()) {
            wv.set_bounds(self.content_rect(), self.ui.window().scale_factor() as f64);
        }
    }

    fn focus_address(&self) {
        if let Some(wv) = &self.tabs[self.active].wv {
            wv.focus_parent();
        }
        self.ui.set_address(self.tabs[self.active].url.clone().into());
        self.ui.invoke_focus_address();
    }

    fn new_tab(&mut self, url: Option<String>) {
        self.new_tab_at(self.tabs.len(), url);
    }

    fn new_tab_at(&mut self, at: usize, url: Option<String>) {
        let id = self.next_id;
        self.next_id += 1;
        let at = at.min(self.tabs.len()).max(self.tabs.iter().filter(|t| t.pinned).count());
        self.tabs.insert(at, Tab::new(id));
        self.active = at;
        let url = url.or_else(|| (!self.cfg.home.is_empty()).then(|| self.cfg.home.clone()));
        self.show_active();
        if let Some(u) = url {
            self.navigate_active(u);
        }
    }

    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs[idx].pinned {
            self.toast("pinned tabs stay open · unpin it first", None, Tone::Warn);
            return;
        }
        let t = self.tabs.remove(idx);
        self.ledger.forget_tab(t.id);
        self.remember_closed(&t);
        let what = if t.url.is_empty() {
            None
        } else {
            Some(urlparts::split(&t.url).map_or(t.url.clone(), |p| p.host.trim_start_matches("www.").to_string()))
        };
        drop(t);
        if self.tabs.is_empty() {
            self.new_tab(None);
        } else {
            let closed_active = self.active == idx;
            if self.active >= idx && self.active > 0 {
                self.active -= 1;
            }
            if closed_active && self.cfg.tab_close == "last" {
                if let Some(i) = self.mru.iter().find_map(|id| self.tabs.iter().position(|t| t.id == *id)) {
                    self.active = i;
                }
            }
            self.show_active();
        }
        // closing never asks; it can always be undone
        if let Some(host) = what {
            self.toast(format!("closed {host}"), Some(ToastAct::Reopen), Tone::Plain);
        }
    }

    fn activate(&mut self, idx: usize) {
        self.hide_card();
        if idx < self.tabs.len() && idx != self.active {
            self.active = idx;
            self.show_active();
        }
    }

    fn apply_theme(&mut self) {
        let p = &THEMES[self.theme];
        paint(&self.ui.global::<Theme>(), p, &self.cfg.font);
        paint(&self.menu.win.global::<Theme>(), p, &self.cfg.font);
        self.menu.win.set_theme_index(self.theme as i32);
        if let Some(h) = self.hwnd() {
            platform::style_frame(h, p.dark, p.bg3);
        }
        for t in &self.tabs {
            if let Some(wv) = &t.wv {
                wv.set_dark(p.dark);
            }
        }
    }

    fn set_theme(&mut self, i: usize) {
        self.theme = i % THEMES.len();
        self.cfg.theme = THEMES[self.theme].id.to_string();
        self.cfg.save();
        self.apply_theme();
    }

    fn blocked_page(&self, url: &str, rule: &str) -> String {
        let p = &THEMES[self.theme];
        let host = urlparts::split(url).map_or(url, |p| p.host);
        format!(
            "<!doctype html><meta charset=utf-8><title>blocked</title>\
             <body style=\"margin:0;height:100vh;display:flex;align-items:flex-end;background:#{:06x};color:#{:06x};font:12px {},monospace\">\
             <div style=\"width:100%;padding:16px 24px 24px;border-top:1px solid #{:06x}\">\
             <div style=\"font-weight:700\">blocked · {}</div>\
             <div style=\"margin-top:10px;color:#{:06x}\">navigation layer · rule {} · add <span style=\"color:#{:06x}\">@@||{}^</span> to filters.txt to allow</div>\
             </div></body>",
            p.bg0,
            p.fg,
            html_escape(&self.cfg.font),
            p.bg3,
            html_escape(host),
            p.mid,
            html_escape(rule),
            p.acc,
            html_escape(host)
        )
    }

    // ------------------------------------------------------------- handlers

    fn run(&mut self, c: Command) {
        match c {
            Command::Address => self.focus_address(),
            Command::NewTab => self.new_tab(None),
            Command::CloseTab => self.close_tab(self.active),
            Command::NextTab => self.activate((self.active + 1) % self.tabs.len()),
            Command::PrevTab => self.activate((self.active + self.tabs.len() - 1) % self.tabs.len()),
            Command::Jump(i) => self.activate(i.min(self.tabs.len() - 1)),
            Command::Back => {
                if let Some(wv) = &self.tabs[self.active].wv {
                    wv.go_back();
                }
            }
            Command::Forward => {
                if let Some(wv) = &self.tabs[self.active].wv {
                    wv.go_forward();
                }
            }
            Command::Reload if self.tabs[self.active].wv.is_none() && self.tabs[self.active].crashed => {
                let url = self.tabs[self.active].url.clone();
                self.tabs[self.active].crashed = false;
                self.navigate_active(url);
            }
            Command::Reload => {
                if self.tabs[self.active].crashed {
                    self.tabs[self.active].crashed = false;
                }
                let t = &self.tabs[self.active];
                if let Some(wv) = &t.wv {
                    if t.loading {
                        wv.stop();
                    } else {
                        wv.reload();
                    }
                }
            }
            Command::Panel => {
                self.panel = !self.panel;
                self.ui.set_panel_open(self.panel);
            }
            Command::Theme => self.set_theme((self.theme + 1) % THEMES.len()),
            Command::Quit => {
                let _ = slint::quit_event_loop();
            }
            Command::Reopen => self.reopen(),
            Command::Mute => self.tab_op(TabOp::ToggleMute, self.active),
            Command::LastTab => self.activate(self.tabs.len() - 1),
            Command::SetTheme(i) => self.set_theme(i),
            Command::CopyLink => self.tab_op(TabOp::CopyLink, self.active),
            Command::Palette => self.open_palette(),
            Command::History => self.open_internal(Internal::History),
            Command::Settings => self.open_internal(Internal::Settings),
            Command::Ephemeral => self.spawn_ephemeral(),
        }
    }

    pub fn handle(&mut self, m: Msg) {
        // Until startup has opened the first tab, only window-level events apply.
        if self.tabs.is_empty() && !matches!(m, Msg::Startup(_) | Msg::Geometry | Msg::Chrome(_) | Msg::Shield(_)) {
            return;
        }
        match m {
            Msg::Startup(urls) => {
                if self.hwnd().is_none() && cfg!(windows) {
                    // window not realised yet; try again shortly
                    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
                        post(Msg::Startup(urls))
                    });
                    return;
                }
                if let Some(h) = self.hwnd() {
                    self.titlebar_ok = platform::install_titlebar(h, |e| post(Msg::Chrome(e)));
                    self.ui.set_zoomed(platform::is_maximized(h));
                }
                self.apply_decorations();
                self.ui.set_eph(self.cfg.ephemeral);
                self.apply_theme();
                let mut urls = urls;
                if urls.is_empty() && !self.cfg.ephemeral {
                    match self.cfg.start.as_str() {
                        "session" => {
                            let saved = std::fs::read_to_string(Self::session_path()).unwrap_or_default();
                            urls = serde_json::from_str::<Vec<String>>(&saved).unwrap_or_default();
                        }
                        "home" if !self.cfg.home.is_empty() => urls = vec![self.cfg.home.clone()],
                        _ => {}
                    }
                }
                if urls.is_empty() {
                    self.new_tab(None);
                } else {
                    for u in urls {
                        let target = omnibox::resolve(&u, &self.cfg.search);
                        self.new_tab(target);
                    }
                    self.activate(0);
                }
            }
            Msg::Key(combo) => {
                if let Some(c) = Command::parse(&combo) {
                    self.run(c);
                }
            }
            Msg::Geometry => {
                let w = self.ui.window().size().width as f32 / self.ui.window().scale_factor();
                self.narrow = w < 720.0;
                self.ui.set_narrow(self.narrow);
                self.layout();
                self.update_hitmap();
            }
            Msg::Chrome(e) => match e {
                ChromeEvent::MaxHot(on) => self.ui.set_max_hot(on),
                ChromeEvent::Active(on) => {
                    self.ui.set_window_active(on);
                    if on && self.menu.open && self.menu.opened_at.elapsed().as_millis() > 80 {
                        self.close_menu();
                    }
                }
                ChromeEvent::Maximized(on) => {
                    self.ui.set_zoomed(on);
                    self.update_hitmap();
                }
            },
            Msg::WinMinimize => self.ui.window().set_minimized(true),
            Msg::WinMaximize => match self.hwnd() {
                Some(h) if cfg!(windows) => platform::toggle_maximize(h),
                _ => {
                    let w = self.ui.window();
                    w.set_maximized(!w.is_maximized());
                }
            },
            Msg::WinClose => {
                let _ = slint::quit_event_loop();
            }
            Msg::MenuToggle => {
                // A click on the menu cell while the menu is open first
                // deactivates (closes) the menu; do not reopen on that click.
                let just_closed = self.menu.closed_at.is_some_and(|t| t.elapsed().as_millis() < 250);
                if self.menu.open {
                    self.close_menu();
                } else if !just_closed {
                    self.open_main_menu();
                }
            }
            Msg::MenuClose => {
                if self.menu.opened_at.elapsed().as_millis() > 80 {
                    self.close_menu();
                }
            }
            Msg::MenuPick(i) => {
                let act = self.menu.acts.get(i).copied().unwrap_or(MenuAct::Nothing);
                self.close_menu();
                match act {
                    MenuAct::Run(c) => self.run(c),
                    MenuAct::Tab(op, t) => self.tab_op(op, t),
                    MenuAct::Nothing => {}
                }
            }
            Msg::TabClose(i) => {
                self.hide_card();
                self.close_tab(i);
            }
            Msg::TabMute(i) => self.tab_op(TabOp::ToggleMute, i),
            Msg::TabMenu(i) => self.open_tab_menu(i),
            Msg::TabHover(i, on) => self.on_tab_hover(i, on),
            Msg::TabDrag(i, x) => self.tab_drag(i, x),
            Msg::TabDrop(i, x) => self.tab_drop(i, x),
            Msg::TabGeom(i, x, w) => self.set_geom(i, x, w),
            Msg::StripStep(d) => self.strip_step(d),
            Msg::StripWheel(d) => self.strip_wheel(d),
            Msg::CardShow(seq) => self.card_timer(seq, true),
            Msg::CardHide(seq) => self.card_timer(seq, false),
            Msg::ToastAction => self.toast_action(),
            Msg::ToastHide(seq) => self.hide_toast(Some(seq)),
            Msg::Audio(id, on) => self.on_audio(id, on),
            Msg::Crashed(id, all) => self.on_crash(id, all),
            Msg::Tick => self.tick(),
            Msg::PaletteEdited(q) => self.fill_palette(&q),
            Msg::PaletteRun(i) => self.palette_run(i),
            Msg::PaletteClose => {
                if self.palette.opened_at.elapsed().as_millis() > 80 {
                    self.close_palette();
                }
            }
            Msg::HistSearch(q) => self.fill_history(&q),
            Msg::HistOpen(i) => self.hist_open(i),
            Msg::HistForget(i) => self.hist_forget(i),
            Msg::HistClear => self.hist_clear(),
            Msg::SetSection(i) => {
                self.settings_section = i;
                self.fill_settings();
            }
            Msg::SetValue(k, v) => self.set_value(&k, &v),
            Msg::ReopenRecent(i) => {
                if i < self.closed.len() {
                    if let Some((url, _)) = self.closed.remove(i) {
                        let t = &self.tabs[self.active];
                        if t.wv.is_none() && t.url.is_empty() {
                            self.navigate_active(url);
                        } else {
                            self.new_tab(Some(url));
                        }
                    }
                }
            }
            Msg::MenuTheme(i) => {
                self.set_theme(i);
                if self.tabs[self.active].page == Some(Internal::Settings) {
                    self.fill_settings();
                }
            }
            Msg::Submit(input) => {
                let sel = self.ui.get_selected().max(0) as usize;
                let target = self
                    .sugg_targets
                    .get(sel)
                    .cloned()
                    .or_else(|| omnibox::resolve(&input, &self.cfg.search).map(Target::Url));
                self.sugg_targets.clear();
                self.ui.set_suggestions(ModelRc::default());
                self.ui.set_strip_preview(SharedString::new());
                match target {
                    Some(Target::Url(t)) => {
                        self.tabs[self.active].strips.set(0);
                        self.navigate_active(t);
                    }
                    Some(Target::Tab(id)) => {
                        if let Some(i) = self.tab_index(id) {
                            self.activate(i);
                        }
                    }
                    Some(Target::Cmd(c)) => {
                        self.page_focus();
                        self.run(c);
                    }
                    None => {}
                }
            }
            Msg::Edited(text) => self.update_suggestions(&text),
            Msg::Complete(i) => {
                let text = match self.sugg_targets.get(i) {
                    Some(Target::Url(u)) => u.clone(),
                    Some(Target::Tab(id)) => self.tab_index(*id).map_or(String::new(), |i| self.tabs[i].url.clone()),
                    Some(Target::Cmd(c)) => {
                        let label = commands::table().into_iter().find(|n| n.cmd == *c).map_or("", |n| n.label);
                        format!(">{label}")
                    }
                    None => String::new(),
                };
                if !text.is_empty() {
                    self.ui.set_address(text.into());
                }
            }
            Msg::TabClicked(i) => self.activate(i),
            Msg::NewTab => self.run(Command::NewTab),
            Msg::Back => self.run(Command::Back),
            Msg::Forward => self.run(Command::Forward),
            Msg::Reload => self.run(Command::Reload),
            Msg::TogglePanel => self.run(Command::Panel),
            Msg::SetShield(on) => {
                #[cfg(feature = "engine-cef")]
                engine::cef::set_shield(on);
                self.shield.set_enabled(on);
                self.cfg.shield = on;
                self.cfg.save();
            }
            Msg::PageFocus => self.page_focus(),
            Msg::Title(id, title) => {
                if let Some(i) = self.tab_index(id) {
                    self.tabs[i].title = title;
                }
            }
            Msg::LoadStarted(id, url) => {
                if let Some(i) = self.tab_index(id) {
                    self.tabs[i].loading = true;
                    self.tabs[i].progress = 0.0;
                    if !url.starts_with("about:") && !url.starts_with("data:") {
                        self.tabs[i].url = url;
                        self.tabs[i].blocked = false;
                    }
                    self.ledger.reset_tab(id);
                }
            }
            Msg::LoadFinished(id, url) => {
                if let Some(i) = self.tab_index(id) {
                    let t = &mut self.tabs[i];
                    t.loading = false;
                    t.strips.set(0);
                    if url.starts_with("http") {
                        t.url = url.clone();
                        let title = t.title.clone();
                        if !self.cfg.ephemeral {
                            self.history.retain(|v| v.url != url);
                            self.history.push(Visit { url, title, at: platform::now_local() });
                        }
                        if self.history.len() > 500 {
                            self.history.remove(0);
                        }
                    }
                }
            }
            Msg::OpenTab(url) => self.new_tab(Some(url)),
            Msg::Redirect(id, url) => {
                if let Some(i) = self.tab_index(id) {
                    if let Some(wv) = &self.tabs[i].wv {
                        wv.load_url(&url);
                    }
                }
            }
            Msg::BlockedNav(id, url, rule) => {
                if let Some(i) = self.tab_index(id) {
                    let html = self.blocked_page(&url, &rule);
                    let t = &mut self.tabs[i];
                    t.url = url;
                    t.loading = false;
                    t.blocked = true;
                    if let Some(wv) = &t.wv {
                        wv.load_html(&html);
                    }
                }
            }
            Msg::Ipc(id, body) => self.on_ipc(id, &body),
            Msg::Shield(ev) => {
                if self.trace {
                    eprintln!("[shield] tab{} {} {}{}", ev.tab, ev.layer.letter(), ev.subject, ev.detail);
                }
                self.ledger.record(&ev);
            }
            Msg::Progress(id, p) => {
                if let Some(i) = self.tab_index(id) {
                    self.tabs[i].progress = p;
                }
            }
            Msg::WebFocus => {
                self.ui.invoke_blur_address();
                self.ui.set_suggestions(ModelRc::default());
                self.sugg_targets.clear();
            }
        }
    }

    fn on_ipc(&mut self, id: u64, body: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return };
        if let Some(key) = v.get("key").and_then(|k| k.as_str()) {
            if let Some(c) = Command::parse(key) {
                self.run(c);
            }
            return;
        }
        if let Some(batch) = v.get("shield").and_then(|b| b.as_array()) {
            for item in batch.iter().take(200) {
                let Some(a) = item.as_array() else { continue };
                let s = |i: usize| a.get(i).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let Some(layer) = Layer::from_letter(&s(0)) else { continue };
                shield::emit(ShieldEvent { tab: id, layer, subject: s(1), detail: format!(" {}", s(2)), count: 1, sticky: false });
            }
        }
    }

    fn page_focus(&mut self) {
        self.ui.set_suggestions(ModelRc::default());
        self.sugg_targets.clear();
        let t = &self.tabs[self.active];
        self.ui.set_address(t.url.clone().into());
        self.ui.invoke_blur_address();
        if let Some(wv) = &t.wv {
            wv.focus();
        }
    }

    fn update_suggestions(&mut self, text: &str) {
        // `>` turns the address bar into a command line
        if let Some(q) = text.trim_start().strip_prefix('>') {
            let mut rows = Vec::new();
            self.sugg_targets.clear();
            for c in commands::table().into_iter().filter(|c| commands::matches(c.label, q)).take(6) {
                rows.push(Suggestion { tag: "cmd".into(), text: c.label.into(), note: c.key.into() });
                self.sugg_targets.push(Target::Cmd(c.cmd));
            }
            self.ui.set_suggestions(ModelRc::new(VecModel::from(rows)));
            self.ui.set_strip_preview(SharedString::new());
            self.ui.set_strip_note(SharedString::new());
            return;
        }
        let pairs: Vec<(String, String)> = self.history.iter().map(|v| (v.url.clone(), v.title.clone())).collect();
        let s = omnibox::suggest(text, &self.cfg.search, &pairs, self.shield.enabled());
        let mut rows: Vec<Suggestion> = Vec::new();
        self.sugg_targets.clear();
        let needle = text.trim().to_lowercase();
        // an open tab beats opening a duplicate: list matches right after "go"
        let open: Vec<(usize, u64, String, String)> = if needle.is_empty() {
            Vec::new()
        } else {
            self.tabs
                .iter()
                .enumerate()
                .filter(|(i, t)| {
                    *i != self.active
                        && !t.url.is_empty()
                        && (t.url.to_lowercase().contains(&needle) || t.title.to_lowercase().contains(&needle))
                })
                .take(3)
                .map(|(i, t)| (i, t.id, t.title.clone(), t.url.clone()))
                .collect()
        };
        let open_urls: Vec<&str> = open.iter().map(|o| o.3.as_str()).collect();
        let mut tabs_added = false;
        for r in s.rows {
            if r.tag == "hist" && open_urls.contains(&r.target.as_str()) {
                continue;
            }
            if r.tag != "go" && !tabs_added {
                for (i, id, title, url) in &open {
                    let host = urlparts::split(url).map_or("", |p| p.host).to_string();
                    rows.push(Suggestion { tag: "tab".into(), text: format!("{}  {}  — {}", i + 1, title, host).into(), note: "switch".into() });
                    self.sugg_targets.push(Target::Tab(*id));
                }
                tabs_added = true;
            }
            rows.push(Suggestion { tag: r.tag.into(), text: r.text.into(), note: r.note.into() });
            self.sugg_targets.push(Target::Url(r.target));
        }
        self.ui.set_suggestions(ModelRc::new(VecModel::from(rows)));
        if s.stripped.is_empty() {
            self.ui.set_strip_preview(SharedString::new());
            self.ui.set_strip_note(SharedString::new());
        } else {
            let n = s.stripped.len();
            self.ui.set_strip_preview(s.stripped.join("  ").into());
            self.ui.set_strip_note(format!("{n} param{}", if n == 1 { "" } else { "s" }).into());
        }
    }

    // ------------------------------------------------------------- render

    /// Tell the OS which bar pixels drag the window and which cell is
    /// the maximize button. Read after layout so it is always current.
    fn update_hitmap(&self) {
        let ui = &self.ui;
        platform::set_hitmap(platform::HitMap {
            scale: ui.window().scale_factor(),
            bar_h: 32.0,
            drags: [(ui.get_drag1_x(), ui.get_drag1_w()), (ui.get_drag2_x(), ui.get_drag2_w())],
            max_btn: if ui.get_controls() { (ui.get_max_x(), ui.get_max_w()) } else { (0.0, 0.0) },
        });
    }

    pub fn refresh(&mut self) {
        self.update_hitmap();
        // uses the cell geometry Slint reported for the last layout
        if self.drag.is_none() {
            self.update_strip();
        }
        let ui = &self.ui;
        let a = self.active.min(self.tabs.len().saturating_sub(1));
        let Some(t) = self.tabs.get(a) else { return };

        let tabs: Vec<TabItem> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let n = i + 1;
                let label = if i == a {
                    let name = if !tab.title.is_empty() {
                        tab.title.clone()
                    } else if tab.url.is_empty() {
                        "empty".into()
                    } else {
                        urlparts::split(&tab.url).map_or(tab.url.clone(), |p| p.host.trim_start_matches("www.").to_string())
                    };
                    let short: String = name.chars().take(if self.narrow { 10 } else { 24 }).collect();
                    format!("{n} {short}")
                } else {
                    n.to_string()
                };
                TabItem {
                    label: label.into(),
                    active: i == a,
                    loading: tab.loading,
                    pinned: tab.pinned,
                    audio: tab.audio || tab.muted,
                    muted: tab.muted,
                    trimmed: tab.trimmed,
                    crashed: tab.crashed,
                }
            })
            .collect();
        ui.set_tabs(ModelRc::new(VecModel::from(tabs)));

        let blank = t.wv.is_none() && t.page.is_none();
        ui.set_blank(blank);
        ui.set_page(match t.page {
            Some(Internal::History) => 1,
            Some(Internal::Settings) => 2,
            None => 0,
        });
        let recent: Vec<SharedString> = self
            .closed
            .iter()
            .take(5)
            .map(|(u, title)| if title.is_empty() { u.as_str().into() } else { title.as_str().into() })
            .collect();
        ui.set_recent(ModelRc::new(VecModel::from(recent)));
        ui.set_loading(t.loading);
        ui.set_progress(if !t.loading { 1.0 } else if t.progress > 0.0 { t.progress } else { 0.35 });
        let (back, fwd) = t.wv.as_ref().map_or((false, false), |wv| (wv.can_go_back(), wv.can_go_forward()));
        ui.set_can_back(back);
        ui.set_can_forward(fwd);
        let parts = urlparts::split(&t.url);
        let host = parts.map_or("", |p| p.host);
        let tag = if t.page.is_some() {
            "brx"
        } else if t.blocked {
            "blocked"
        } else if t.url.starts_with("https://") {
            "tls"
        } else if t.url.starts_with("http://") {
            "http"
        } else {
            ""
        };
        ui.set_security(tag.into());
        // look-alike domains: punycode labels are shown as-is, and flagged
        ui.set_addr_puny(host.split('.').any(|l| l.starts_with("xn--")));
        let display = t.page.map_or(t.url.as_str(), |p| p.url());
        if t.page.is_some() || parts.is_none() {
            ui.set_addr_host(display.into());
            ui.set_addr_rest(SharedString::new());
        } else if let Some(p) = parts {
            let start = display.find(p.host).map_or(0, |i| i + p.host.len());
            ui.set_addr_host(p.host.into());
            ui.set_addr_rest(if self.narrow { SharedString::new() } else { display[start..].into() });
        }
        if !ui.get_address_focused() {
            ui.set_address(display.into());
        }
        let title = if t.title.is_empty() { "brxswer".to_string() } else { format!("{} — brxswer", t.title) };
        ui.set_win_title(title.into());

        let on = self.shield.enabled();
        ui.set_shield_on(on);
        ui.set_shield_count(self.ledger.tab_total(t.id).to_string().into());
        ui.set_panel_open(self.panel);
        let host = urlparts::split(&t.url).map_or("—".to_string(), |p| p.host.to_string());
        ui.set_panel_host(host.into());
        let tl = self.ledger.tab(t.id);
        let layers: Vec<LayerRow> = Layer::ALL
            .iter()
            .map(|l| LayerRow {
                letter: l.letter().into(),
                label: l.label().into(),
                count: tl.map_or(0, |x| x.counts[l.index()]).to_string().into(),
            })
            .collect();
        ui.set_layers(ModelRc::new(VecModel::from(layers)));
        let log: Vec<LogRow> = tl
            .map(|x| {
                x.log
                    .iter()
                    .take(40)
                    .map(|(l, s, d)| LogRow { letter: l.letter().into(), subject: s.into(), detail: d.into() })
                    .collect()
            })
            .unwrap_or_default();
        ui.set_log(ModelRc::new(VecModel::from(log)));

        let s = &self.ledger.session;
        let blocked = s[Layer::Transport.index()] + s[Layer::Runtime.index()] + s[Layer::Navigation.index()];
        ui.set_session_line(
            format!(
                "session · {} blocked · {} params stripped · {} beacons faked",
                group(blocked),
                group(s[Layer::Params.index()]),
                group(s[Layer::Beacon.index()])
            )
            .into(),
        );
        ui.set_session_short(format!("session {}", group(s.iter().sum())).into());
        let n = self.tabs.len();
        let bg = self.tabs.iter().enumerate().filter(|(i, t)| *i != a && t.wv.is_some()).count();
        ui.set_footer_right(format!("{n} tab{} · {bg} trimmed", if n == 1 { "" } else { "s" }).into());
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Navigation layer. Runs synchronously inside the engine's
/// navigation-starting event: decide, emit, post — never touch app state.
fn navigation_layer(tab: u64, shield: &Shield, strips: &Cell<u8>, https: bool, url: &str) -> bool {
    if https {
        if let Some(rest) = url.strip_prefix("http://") {
            let host = urlparts::split(url).map_or("", |p| p.host);
            let local = host == "localhost" || host.ends_with(".localhost") || host.ends_with(".test") || host.parse::<std::net::IpAddr>().is_ok();
            if !local {
                post(Msg::Redirect(tab, format!("https://{rest}")));
                return false;
            }
        }
    }
    if !shield.enabled() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return true;
    }
    if let Verdict::Block(rule) = shield.check(url) {
        let host = urlparts::split(url).map_or(String::new(), |p| p.host.to_string());
        shield::emit(ShieldEvent { tab, layer: Layer::Navigation, subject: host, detail: " page refused".into(), count: 1, sticky: true });
        post(Msg::BlockedNav(tab, url.to_string(), rule));
        return false;
    }
    // A server that keeps redirecting back to the tracked URL wins after 3 tries.
    if strips.get() < 3 {
        if let Some(c) = params::clean_navigation(url) {
            strips.set(strips.get() + 1);
            shield::emit(ShieldEvent {
                tab,
                layer: Layer::Params,
                subject: c.removed.join(", "),
                detail: " stripped".into(),
                count: c.removed.len() as u32,
                sticky: true,
            });
            post(Msg::Redirect(tab, c.url));
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(Command::parse("ctrl+3"), Some(Command::Jump(2)));
        assert_eq!(Command::parse("ctrl+0"), None);
        assert_eq!(Command::parse("ctrl+shift+Tab"), Some(Command::PrevTab));
        assert_eq!(Command::parse("ctrl+x"), None);
        assert_eq!(group(2904), "2,904");
        assert_eq!(group(12), "12");
    }
}
