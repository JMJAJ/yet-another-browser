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
use crate::{AppWindow, LayerRow, LogRow, MenuRow, MenuWindow, Suggestion, TabItem, Theme};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};

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
    } else if c.is_ascii_alphanumeric() {
        c.to_ascii_lowercase().to_string()
    } else {
        return None;
    };
    let shift = shift || c == char::from(Key::Backtab);
    let s = format!(
        "{}{}{}{}",
        if ctrl { "ctrl+" } else { "" },
        if alt { "alt+" } else { "" },
        if shift && key == "Tab" { "shift+" } else { "" },
        key
    );
    Command::parse(&s).map(|_| s)
}

// ------------------------------------------------------------------ messages

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
        while let Some(m) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
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
    wv: Option<WebView>,
    url: String,
    title: String,
    loading: bool,
    /// Parameter-strip redirects since the last finished load (loop guard).
    strips: Rc<Cell<u8>>,
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
    history: Vec<(String, String)>,
    sugg_targets: Vec<String>,
    menu: Menu,
}

/// What a main-menu row does.
#[derive(Clone, Copy)]
enum MenuAct {
    Run(Command),
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
        let ctx = WebContext::new(Some(crate::config::dir().join("webdata")));
        let theme = theme::index_of(&cfg.theme);
        let menu_win = MenuWindow::new().expect("menu window");
        menu_win.on_pick(|i| post(Msg::MenuPick(i.max(0) as usize)));
        menu_win.on_pick_theme(|i| post(Msg::MenuTheme(i.max(0) as usize)));
        menu_win.on_dismiss(|| post(Msg::MenuClose));
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
            sugg_targets: Vec::new(),
            menu: Menu {
                win: menu_win,
                owned: false,
                open: false,
                opened_at: std::time::Instant::now(),
                closed_at: None,
                acts: Vec::new(),
            },
        }
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

    // ------------------------------------------------------------- menus

    fn open_menu(&mut self) {
        let rows: Vec<(u8, &str, &str, MenuAct)> = vec![
            (0, "new tab", "ctrl+t", MenuAct::Run(Command::NewTab)),
            (0, "close tab", "ctrl+w", MenuAct::Run(Command::CloseTab)),
            (1, "", "", MenuAct::Nothing),
            (2, "theme", "", MenuAct::Nothing),
            (1, "", "", MenuAct::Nothing),
            (0, "shield", "ctrl+b", MenuAct::Run(Command::Panel)),
            (0, "next theme", "alt+t", MenuAct::Run(Command::Theme)),
            (1, "", "", MenuAct::Nothing),
            (0, "quit", "ctrl+q", MenuAct::Run(Command::Quit)),
        ];
        let height: f32 = 12.0 + rows.iter().map(|r| match r.0 { 1 => 9.0, 2 => 30.0, _ => 26.0 }).sum::<f32>();
        let nav: Vec<i32> = rows.iter().enumerate().filter(|(_, r)| r.0 == 0).map(|(i, _)| i as i32).collect();
        self.menu.acts = rows.iter().map(|r| r.3).collect();
        let model: Vec<MenuRow> = rows
            .iter()
            .map(|r| MenuRow { kind: r.0 as i32, label: r.1.into(), key: r.2.into(), enabled: true })
            .collect();
        let m = &self.menu.win;
        m.set_rows(ModelRc::new(VecModel::from(model)));
        m.set_nav(ModelRc::new(VecModel::from(nav)));
        m.set_menu_w(280.0);
        m.set_menu_h(height);
        m.set_swatches(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.acc)).collect::<Vec<_>>())));
        m.set_swatch_bgs(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.bg0)).collect::<Vec<_>>())));
        m.set_theme_index(self.theme as i32);
        m.set_hl(-1);
        m.set_pos(-1);
        paint(&m.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);

        // the menu's right edge lines up with the right edge of the menu cell
        let scale = self.ui.window().scale_factor();
        let x = (self.ui.get_menu_btn_x() + self.ui.get_menu_btn_w() - 280.0).max(0.0);
        let y = 32.0;
        let (ox, oy) = match self.hwnd() {
            Some(h) if cfg!(windows) => platform::client_origin(h),
            _ => {
                let p = self.ui.window().position();
                (p.x, p.y)
            }
        };
        m.window().set_position(slint::PhysicalPosition::new(ox + (x * scale) as i32, oy + (y * scale) as i32));
        let _ = m.show();
        if let (Some(popup), Some(owner)) = (hwnd_of(m.window()), self.hwnd()) {
            if !self.menu.owned {
                platform::own_popup(popup, owner, || post(Msg::MenuClose));
                self.menu.owned = true;
            }
            platform::activate(popup);
        }
        m.invoke_take_keys();
        self.menu.open = true;
        self.menu.opened_at = std::time::Instant::now();
        self.ui.set_menu_open(true);
    }

    fn close_menu(&mut self) {
        if self.menu.open {
            let _ = self.menu.win.hide();
            self.menu.open = false;
            self.menu.closed_at = Some(std::time::Instant::now());
            self.ui.set_menu_open(false);
        }
    }

    // ------------------------------------------------------------- webviews

    fn create_webview(&mut self, idx: usize, url: &str) -> bool {
        let id = self.tabs[idx].id;
        let strips = self.tabs[idx].strips.clone();
        let nav_shield = self.shield.clone();
        let rect = self.content_rect();
        let p = &THEMES[self.theme];
        let handle = self.ui.window().window_handle();
        let builder = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_bounds(rect)
            .with_url(url)
            .with_initialization_script_for_main_only(self.script.clone(), false)
            .with_ipc_handler(move |req| post(Msg::Ipc(id, req.into_body())))
            .with_navigation_handler(move |url| navigation_layer(id, &nav_shield, &strips, &url))
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
            .with_additional_browser_args(platform::BROWSER_ARGS)
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
        self.tabs[idx].wv = Some(wv);
        true
    }

    fn navigate_active(&mut self, url: String) {
        let idx = self.active;
        self.tabs[idx].url = url.clone();
        self.tabs[idx].loading = true;
        match &self.tabs[idx].wv {
            Some(wv) => {
                let _ = wv.load_url(&url);
                let _ = wv.set_visible(true);
                let _ = wv.focus();
            }
            None => {
                self.create_webview(idx, &url);
            }
        }
        self.ui.invoke_blur_address();
        self.layout();
    }

    fn show_active(&mut self) {
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(wv) = &t.wv {
                let on = i == self.active;
                let _ = wv.set_visible(on);
                platform::set_background(wv, !on);
            }
        }
        self.layout();
        match &self.tabs[self.active].wv {
            Some(wv) => {
                self.ui.invoke_blur_address();
                let _ = wv.focus();
            }
            None => self.focus_address(),
        }
    }

    fn layout(&self) {
        // geometry events can arrive before the first tab exists
        if let Some(wv) = self.tabs.get(self.active).and_then(|t| t.wv.as_ref()) {
            let _ = wv.set_bounds(self.content_rect());
        }
    }

    fn focus_address(&self) {
        if let Some(wv) = &self.tabs[self.active].wv {
            let _ = wv.focus_parent();
        }
        self.ui.set_address(self.tabs[self.active].url.clone().into());
        self.ui.invoke_focus_address();
    }

    fn new_tab(&mut self, url: Option<String>) {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            wv: None,
            url: String::new(),
            title: String::new(),
            loading: false,
            strips: Rc::new(Cell::new(0)),
        });
        self.active = self.tabs.len() - 1;
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
        let t = self.tabs.remove(idx);
        self.ledger.forget_tab(t.id);
        drop(t);
        if self.tabs.is_empty() {
            self.new_tab(None);
            return;
        }
        if self.active >= idx && self.active > 0 {
            self.active -= 1;
        }
        self.show_active();
    }

    fn activate(&mut self, idx: usize) {
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
                platform::set_web_theme(wv, p.dark);
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
                    let _ = wv.go_back();
                }
            }
            Command::Forward => {
                if let Some(wv) = &self.tabs[self.active].wv {
                    let _ = wv.go_forward();
                }
            }
            Command::Reload => {
                let t = &self.tabs[self.active];
                if let Some(wv) = &t.wv {
                    if t.loading {
                        let _ = wv.evaluate_script("window.stop()");
                    } else {
                        let _ = wv.reload();
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
                    let custom = platform::install_titlebar(h, |e| post(Msg::Chrome(e)));
                    self.ui.set_controls(custom && platform::CUSTOM_TITLEBAR);
                    self.ui.set_zoomed(platform::is_maximized(h));
                }
                self.apply_theme();
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
                    self.open_menu();
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
                if let MenuAct::Run(c) = act {
                    self.run(c);
                }
            }
            Msg::MenuTheme(i) => self.set_theme(i),
            Msg::Submit(input) => {
                let sel = self.ui.get_selected().max(0) as usize;
                let target = self
                    .sugg_targets
                    .get(sel)
                    .cloned()
                    .or_else(|| omnibox::resolve(&input, &self.cfg.search));
                self.sugg_targets.clear();
                self.ui.set_suggestions(ModelRc::default());
                self.ui.set_strip_preview(SharedString::new());
                if let Some(t) = target {
                    self.tabs[self.active].strips.set(0);
                    self.navigate_active(t);
                }
            }
            Msg::Edited(text) => self.update_suggestions(&text),
            Msg::Complete(i) => {
                if let Some(t) = self.sugg_targets.get(i) {
                    self.ui.set_address(t.clone().into());
                }
            }
            Msg::TabClicked(i) => self.activate(i),
            Msg::NewTab => self.run(Command::NewTab),
            Msg::Back => self.run(Command::Back),
            Msg::Forward => self.run(Command::Forward),
            Msg::Reload => self.run(Command::Reload),
            Msg::TogglePanel => self.run(Command::Panel),
            Msg::SetShield(on) => {
                self.shield.set_enabled(on);
                self.cfg.shield = on;
                self.cfg.save();
            }
            Msg::PageFocus => {
                self.ui.set_suggestions(ModelRc::default());
                self.sugg_targets.clear();
                let t = &self.tabs[self.active];
                self.ui.set_address(t.url.clone().into());
                if let Some(wv) = &t.wv {
                    self.ui.invoke_blur_address();
                    let _ = wv.focus();
                }
            }
            Msg::Title(id, title) => {
                if let Some(i) = self.tab_index(id) {
                    self.tabs[i].title = title;
                }
            }
            Msg::LoadStarted(id, url) => {
                if let Some(i) = self.tab_index(id) {
                    self.tabs[i].loading = true;
                    if !url.starts_with("about:") && !url.starts_with("data:") {
                        self.tabs[i].url = url;
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
                        self.history.retain(|(u, _)| u != &url);
                        self.history.push((url, t.title.clone()));
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
                        let _ = wv.load_url(&url);
                    }
                }
            }
            Msg::BlockedNav(id, url, rule) => {
                if let Some(i) = self.tab_index(id) {
                    let html = self.blocked_page(&url, &rule);
                    let t = &mut self.tabs[i];
                    t.url = url;
                    t.loading = false;
                    if let Some(wv) = &t.wv {
                        let _ = wv.load_html(&html);
                    }
                }
            }
            Msg::Ipc(id, body) => self.on_ipc(id, &body),
            Msg::Shield(ev) => self.ledger.record(&ev),
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

    fn update_suggestions(&mut self, text: &str) {
        let s = omnibox::suggest(text, &self.cfg.search, &self.history, self.shield.enabled());
        self.sugg_targets = s.rows.iter().map(|r| r.target.clone()).collect();
        let rows: Vec<Suggestion> = s
            .rows
            .into_iter()
            .map(|r| Suggestion { tag: r.tag.into(), text: r.text.into(), note: r.note.into() })
            .collect();
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
                    let short: String = name.chars().take(24).collect();
                    format!("{n} {short}")
                } else {
                    n.to_string()
                };
                TabItem { label: label.into(), active: i == a, loading: tab.loading }
            })
            .collect();
        ui.set_tabs(ModelRc::new(VecModel::from(tabs)));

        let blank = t.wv.is_none();
        ui.set_blank(blank);
        ui.set_loading(t.loading);
        ui.set_progress(if t.loading { 0.35 } else { 1.0 });
        let (back, fwd) = t.wv.as_ref().map_or((false, false), |wv| {
            (wv.can_go_back().unwrap_or(false), wv.can_go_forward().unwrap_or(false))
        });
        ui.set_can_back(back);
        ui.set_can_forward(fwd);
        ui.set_security(
            if t.url.starts_with("https://") {
                "tls"
            } else if t.url.starts_with("http://") {
                "http"
            } else {
                ""
            }
            .into(),
        );
        if !ui.get_address_focused() {
            ui.set_address(t.url.clone().into());
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
fn navigation_layer(tab: u64, shield: &Shield, strips: &Cell<u8>, url: &str) -> bool {
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
