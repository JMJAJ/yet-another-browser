//! Transient windows over the page: menus, the tab hover card and toasts.
//! Each is an owned popup window; see `platform::own_popup`.

use super::*;
use crate::platform::PopupKind;
use crate::{CardWindow, ToastWindow};

pub(super) struct Card {
    pub win: CardWindow,
    pub owned: bool,
    pub visible: bool,
    /// Bumped on every hover change; a pending show/hide only applies if
    /// the sequence it captured is still current.
    pub seq: u64,
    pub tab: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) enum ToastAct {
    Reopen,
}

pub(super) struct Toast {
    pub win: ToastWindow,
    pub owned: bool,
    pub seq: u64,
    pub act: Option<ToastAct>,
}

#[allow(dead_code)] // Bad is for error toasts (M3)
pub(super) enum Tone {
    Plain,
    Ok,
    Warn,
    Bad,
}

/// A main- or tab-menu row: (kind, label, key, enabled, action).
type Row = (u8, String, &'static str, bool, MenuAct);

fn item(label: impl Into<String>, key: &'static str, act: MenuAct) -> Row {
    (0, label.into(), key, true, act)
}
fn sep() -> Row {
    (1, String::new(), "", true, MenuAct::Nothing)
}

impl App {
    /// Client-area origin of the main window in screen pixels.
    fn origin(&self) -> (i32, i32) {
        match self.hwnd() {
            Some(h) if cfg!(windows) => platform::client_origin(h),
            _ => {
                let p = self.ui.window().position();
                (p.x, p.y)
            }
        }
    }

    /// Screen position for a point given in the main window's logical px.
    pub(super) fn to_screen(&self, x: f32, y: f32) -> slint::PhysicalPosition {
        let s = self.ui.window().scale_factor();
        let (ox, oy) = self.origin();
        slint::PhysicalPosition::new(ox + (x * s) as i32, oy + (y * s) as i32)
    }

    // ---------------------------------------------------------------- menus

    pub(super) fn open_main_menu(&mut self) {
        let rows = vec![
            item("new tab", "ctrl+t", MenuAct::Run(Command::NewTab)),
            item("reopen closed tab", "ctrl+shift+t", MenuAct::Run(Command::Reopen)),
            item("close tab", "ctrl+w", MenuAct::Run(Command::CloseTab)),
            item("new ephemeral window", "ctrl+shift+n", MenuAct::Run(Command::Ephemeral)),
            sep(),
            item("command palette", "ctrl+k", MenuAct::Run(Command::Palette)),
            item("find in page", "ctrl+f", MenuAct::Run(Command::Find)),
            item("history", "ctrl+h", MenuAct::Run(Command::History)),
            item("downloads", "ctrl+j", MenuAct::Run(Command::Downloads)),
            sep(),
            (3, "zoom".into(), "", true, MenuAct::Nothing),
            (2, "theme".into(), "", true, MenuAct::Nothing),
            sep(),
            item("settings", "ctrl+,", MenuAct::Run(Command::Settings)),
            item("shield", "ctrl+b", MenuAct::Run(Command::Panel)),
            sep(),
            item("quit", "ctrl+q", MenuAct::Run(Command::Quit)),
        ];
        let rows: Vec<Row> = rows
            .into_iter()
            .map(|mut r| {
                if matches!(r.4, MenuAct::Run(Command::Reopen)) {
                    r.3 = !self.closed.is_empty();
                }
                r
            })
            .collect();
        // the menu's right edge lines up with the right edge of the menu cell
        let x = (self.ui.get_menu_btn_x() + self.ui.get_menu_btn_w() - 280.0).max(0.0);
        self.show_menu(rows, x, 32.0, 280.0, true);
    }

    pub(super) fn open_tab_menu(&mut self, i: usize) {
        let Some(t) = self.tabs.get(i) else { return };
        let rows = vec![
            item("reload", "ctrl+r", MenuAct::Tab(TabOp::Reload, i)),
            item("duplicate", "", MenuAct::Tab(TabOp::Duplicate, i)),
            item(if t.pinned { "unpin" } else { "pin" }, "", MenuAct::Tab(TabOp::TogglePin, i)),
            (0, if t.muted { "unmute".into() } else { "mute".into() }, "ctrl+m", t.audio || t.muted, MenuAct::Tab(TabOp::ToggleMute, i)),
            (0, "copy clean link".into(), "", !t.url.is_empty(), MenuAct::Tab(TabOp::CopyLink, i)),
            sep(),
            (0, "close".into(), "ctrl+w", !t.pinned, MenuAct::Tab(TabOp::Close, i)),
            (0, "close other tabs".into(), "", self.tabs.len() > 1, MenuAct::Tab(TabOp::CloseOthers, i)),
            (0, "close tabs to the right".into(), "", i + 1 < self.tabs.len(), MenuAct::Tab(TabOp::CloseRight, i)),
            sep(),
            (0, "reopen closed tab".into(), "ctrl+shift+t", !self.closed.is_empty(), MenuAct::Run(Command::Reopen)),
        ];
        let x = self.tab_screen_x(i).unwrap_or(0.0);
        self.show_menu(rows, x, 32.0, 250.0, false);
    }

    pub(super) fn show_menu_rows(&mut self, rows: Vec<Row>, x: f32, y: f32, w: f32) {
        self.show_menu(rows, x, y, w, false);
    }

    fn show_menu(&mut self, rows: Vec<Row>, x: f32, y: f32, w: f32, from_menu_cell: bool) {
        self.hide_card();
        let height: f32 = 12.0 + rows.iter().map(|r| match r.0 { 1 => 9.0, 2 | 3 => 30.0, _ => 26.0 }).sum::<f32>();
        let nav: Vec<i32> = rows.iter().enumerate().filter(|(_, r)| r.0 == 0 && r.3).map(|(i, _)| i as i32).collect();
        self.menu.acts = rows.iter().map(|r| r.4.clone()).collect();
        let model: Vec<MenuRow> = rows
            .iter()
            .map(|r| MenuRow { kind: r.0 as i32, label: r.1.as_str().into(), key: r.2.into(), enabled: r.3 })
            .collect();
        let win_w = self.ui.window().size().width as f32 / self.ui.window().scale_factor();
        let x = x.min(win_w - w - 1.0).max(0.0);
        let pos = self.to_screen(x, y);
        let m = &self.menu.win;
        m.set_rows(ModelRc::new(VecModel::from(model)));
        m.set_nav(ModelRc::new(VecModel::from(nav)));
        m.set_menu_w(w);
        m.set_menu_h(height);
        m.set_swatches(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.acc)).collect::<Vec<_>>())));
        m.set_swatch_bgs(ModelRc::new(VecModel::from(THEMES.iter().map(|t| theme::color(t.bg0)).collect::<Vec<_>>())));
        m.set_theme_index(self.theme as i32);
        m.set_zoom_pct(self.current_zoom() as i32);
        m.set_hl(-1);
        m.set_pos(-1);
        paint(&m.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);
        m.window().set_position(pos);
        let _ = m.show();
        if let (Some(popup), Some(owner)) = (hwnd_of(m.window()), self.hwnd()) {
            if !self.menu.owned {
                platform::own_popup(popup, owner, PopupKind::Menu, || post(Msg::MenuClose));
                self.menu.owned = true;
            }
            platform::activate(popup);
        }
        m.invoke_take_keys();
        self.menu.open = true;
        self.menu.opened_at = std::time::Instant::now();
        self.ui.set_menu_open(from_menu_cell);
    }

    pub(super) fn close_menu(&mut self) {
        if self.menu.open {
            let _ = self.menu.win.hide();
            self.menu.open = false;
            self.menu.closed_at = Some(std::time::Instant::now());
            self.ui.set_menu_open(false);
        }
    }

    // ----------------------------------------------------------- hover card

    pub(super) fn on_tab_hover(&mut self, i: usize, on: bool) {
        self.card.seq += 1;
        let seq = self.card.seq;
        if on {
            self.card.tab = Some(i);
            if self.menu.open || self.drag.is_some() {
                return;
            }
            if self.card.visible {
                // moving along the strip swaps the card without the delay
                self.show_card(i);
            } else {
                slint::Timer::single_shot(std::time::Duration::from_millis(450), move || post(Msg::CardShow(seq)));
            }
        } else {
            self.card.tab = None;
            slint::Timer::single_shot(std::time::Duration::from_millis(80), move || post(Msg::CardHide(seq)));
        }
    }

    pub(super) fn card_timer(&mut self, seq: u64, show: bool) {
        if seq != self.card.seq {
            return;
        }
        match (show, self.card.tab) {
            (true, Some(i)) if !self.menu.open && self.drag.is_none() => self.show_card(i),
            (false, None) => self.hide_card(),
            _ => {}
        }
    }

    fn show_card(&mut self, i: usize) {
        let Some(t) = self.tabs.get(i) else { return };
        let mut state = vec![format!("{} blocked", self.ledger.tab_total(t.id))];
        if t.audio || t.muted {
            state.push(if t.muted { "muted".into() } else { "playing audio".into() });
        }
        if t.trimmed {
            let mins = t.hidden_since.map_or(0, |s| s.elapsed().as_secs() / 60);
            state.push(format!("trimmed, hidden {mins} min"));
        }
        if t.loading {
            state.push("loading".into());
        }
        if t.pinned {
            state.push("pinned".into());
        }
        if t.crashed {
            state.push("stopped".into());
        }
        let c = &self.card.win;
        c.set_heading(if t.title.is_empty() { "empty tab".into() } else { t.title.as_str().into() });
        c.set_url(if t.url.is_empty() { "no page".into() } else { t.url.as_str().into() });
        c.set_state(state.join(" · ").into());
        paint(&c.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);
        let win_w = self.ui.window().size().width as f32 / self.ui.window().scale_factor();
        let w = c.get_want_w();
        let x = self.tab_screen_x(i).unwrap_or(0.0).min(win_w - w - 2.0).max(0.0);
        c.window().set_position(self.to_screen(x, 34.0));
        let _ = c.show();
        if !self.card.owned {
            if let (Some(p), Some(o)) = (hwnd_of(c.window()), self.hwnd()) {
                platform::own_popup(p, o, PopupKind::Card, || {});
                self.card.owned = true;
            }
        }
        self.card.visible = true;
    }

    pub(super) fn hide_card(&mut self) {
        if self.card.visible {
            let _ = self.card.win.hide();
            self.card.visible = false;
        }
    }

    // ---------------------------------------------------------------- toast

    pub(super) fn toast(&mut self, message: impl Into<String>, act: Option<ToastAct>, tone: Tone) {
        self.toast.seq += 1;
        self.toast.act = act;
        let t = &self.toast.win;
        t.set_message(message.into().into());
        t.set_action(match act {
            Some(ToastAct::Reopen) => "reopen · ctrl+shift+t".into(),
            None => SharedString::new(),
        });
        t.set_tone(match tone {
            Tone::Plain => 0,
            Tone::Ok => 1,
            Tone::Warn => 2,
            Tone::Bad => 3,
        });
        paint(&t.global::<Theme>(), &THEMES[self.theme], &self.cfg.font);
        let s = self.ui.window().scale_factor();
        let size = self.ui.window().size();
        let (w, h) = (t.get_want_w(), t.get_want_h());
        let x = size.width as f32 / s - w - 12.0;
        let y = size.height as f32 / s - h - 12.0;
        t.window().set_position(self.to_screen(x.max(0.0), y.max(0.0)));
        let _ = t.show();
        if !self.toast.owned {
            if let (Some(p), Some(o)) = (hwnd_of(t.window()), self.hwnd()) {
                platform::own_popup(p, o, PopupKind::Toast, || {});
                self.toast.owned = true;
            }
        }
        let seq = self.toast.seq;
        slint::Timer::single_shot(std::time::Duration::from_secs(4), move || post(Msg::ToastHide(seq)));
    }

    pub(super) fn hide_toast(&mut self, seq: Option<u64>) {
        if seq.is_none_or(|s| s == self.toast.seq) {
            let _ = self.toast.win.hide();
            self.toast.act = None;
        }
    }

    pub(super) fn toast_action(&mut self) {
        let act = self.toast.act;
        self.hide_toast(None);
        if let Some(ToastAct::Reopen) = act {
            self.run(Command::Reopen);
        }
    }
}
