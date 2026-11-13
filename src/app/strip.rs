//! The tab strip: geometry, drag to reorder, overflow scrolling, tab-menu
//! actions, closing and reopening, audio, crashes and sleeping tabs.

use super::popups::{ToastAct, Tone};
use super::*;

/// Hidden this long, a tab's page is put to sleep.
const TRIM_AFTER: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const CLOSED_KEEP: usize = 25;

#[derive(Clone, Copy, Debug)]
pub(super) enum TabOp {
    Reload,
    Duplicate,
    TogglePin,
    ToggleMute,
    CopyLink,
    Close,
    CloseOthers,
    CloseRight,
}

impl App {
    // -------------------------------------------------------------- geometry

    pub(super) fn set_geom(&mut self, i: usize, x: f32, w: f32) {
        if self.geoms.len() <= i {
            self.geoms.resize(i + 1, (0.0, 0.0));
        }
        self.geoms[i] = (x, w);
    }

    /// Left edge of tab `i` in main-window logical px (after scrolling).
    pub(super) fn tab_screen_x(&self, i: usize) -> Option<f32> {
        let (x, _) = *self.geoms.get(i)?;
        Some(self.ui.get_strip_x() + x - self.strip_scroll)
    }

    fn content_width(&self) -> f32 {
        self.geoms.iter().take(self.tabs.len()).map(|g| g.0 + g.1).fold(0.0, f32::max)
    }

    /// Keep the scroll in range and the active tab in view; work out how
    /// many tabs are hidden on each side for the ‹ n / n › cells.
    pub(super) fn update_strip(&mut self) {
        let view = self.ui.get_strip_w();
        let total = self.content_width();
        let max_scroll = (total - view).max(0.0);
        if let Some(&(x, w)) = self.geoms.get(self.active) {
            // a tab wider than the strip shows its start, never a clipped title
            if x < self.strip_scroll || w > view {
                self.strip_scroll = x;
            } else if x + w > self.strip_scroll + view {
                self.strip_scroll = x + w - view;
            }
        }
        self.strip_scroll = self.strip_scroll.clamp(0.0, max_scroll);
        let total = self.content_width();
        self.publish_strip(view, total);
    }

    pub(super) fn strip_step(&mut self, dir: i32) {
        // scroll by three cells' worth
        let step = self.geoms.iter().take(3).map(|g| g.1).sum::<f32>().max(60.0);
        self.strip_scroll += step * dir as f32;
        let active = self.active;
        self.active = usize::MAX; // don't snap back to the active tab
        self.update_strip_free();
        self.active = active;
    }

    pub(super) fn strip_wheel(&mut self, delta: f32) {
        self.strip_scroll -= delta;
        self.update_strip_free();
    }

    /// Snap the scroll to a cell edge, then report which cells are wholly in
    /// view, how wide they are together, and how many are hidden each side.
    fn publish_strip(&mut self, view: f32, total: f32) {
        let n = self.tabs.len().min(self.geoms.len());
        let cells = &self.geoms[..n];
        if total <= view + 0.5 {
            self.strip_scroll = 0.0;
            self.ui.set_strip_scroll(0.0);
            self.ui.set_strip_show(0.0);
            self.ui.set_hidden_left(0);
            self.ui.set_hidden_right(0);
            return;
        }
        // start at the first cell that begins at or after the scroll position
        if let Some(first) = cells.iter().find(|g| g.0 + 0.5 >= self.strip_scroll) {
            self.strip_scroll = first.0;
        }
        let end = self.strip_scroll + view;
        let shown: Vec<&(f32, f32)> = cells.iter().filter(|g| g.0 + 0.5 >= self.strip_scroll && g.0 + g.1 <= end + 0.5).collect();
        let show_w = shown.last().map_or(view, |g| g.0 + g.1 - self.strip_scroll);
        let left = cells.iter().filter(|g| g.0 + 0.5 < self.strip_scroll).count();
        let right = n - left - shown.len();
        self.ui.set_strip_scroll(self.strip_scroll);
        self.ui.set_strip_show(show_w.max(1.0));
        self.ui.set_hidden_left(left as i32);
        self.ui.set_hidden_right(right as i32);
    }

    fn update_strip_free(&mut self) {
        let view = self.ui.get_strip_w();
        let max_scroll = (self.content_width() - view).max(0.0);
        self.strip_scroll = self.strip_scroll.clamp(0.0, max_scroll);
        let total = self.content_width();
        self.publish_strip(view, total);
    }

    // --------------------------------------------------------------- dragging

    fn drop_index(&self, from: usize, pointer_in_cell: f32) -> usize {
        let Some(&(fx, _)) = self.geoms.get(from) else { return from };
        let x = fx + pointer_in_cell;
        let n = self.tabs.len().min(self.geoms.len());
        (0..n).find(|&j| x < self.geoms[j].0 + self.geoms[j].1 / 2.0).unwrap_or(n)
    }

    pub(super) fn tab_drag(&mut self, from: usize, x: f32) {
        self.hide_card();
        let to = self.drop_index(from, x);
        self.drag = Some((from, to));
        self.ui.set_drag_from(from as i32);
        self.ui.set_drop_at(if to == from || to == from + 1 { -1 } else { to as i32 });
    }

    pub(super) fn tab_drop(&mut self, from: usize, x: f32) {
        let to = self.drop_index(from, x);
        self.drag = None;
        self.ui.set_drag_from(-1);
        self.ui.set_drop_at(-1);
        if from >= self.tabs.len() || to == from || to == from + 1 {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        let at = if to > from { to - 1 } else { to };
        // Dropping inside the pinned group pins; dropping after it unpins.
        let pinned_others = self.tabs.iter().filter(|t| t.pinned).count();
        let was_pinned = tab.pinned;
        self.tabs.insert(at, tab);
        self.tabs[at].pinned = at < pinned_others || (at == pinned_others && was_pinned);
        self.active = self.tabs.iter().position(|t| t.id == active_id).unwrap_or(0);
    }

    // ------------------------------------------------------------ tab actions

    pub(super) fn tab_op(&mut self, op: TabOp, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        match op {
            TabOp::Reload => {
                if let Some(wv) = &self.tabs[i].wv {
                    wv.reload();
                }
            }
            TabOp::Duplicate => {
                let url = self.tabs[i].url.clone();
                if !url.is_empty() {
                    self.new_tab_at(i + 1, Some(url));
                }
            }
            TabOp::TogglePin => {
                let mut t = self.tabs.remove(i);
                let active_id = self.tabs.get(self.active).map(|x| x.id);
                t.pinned = !t.pinned;
                let at = self.tabs.iter().filter(|x| x.pinned).count();
                let id = t.id;
                self.tabs.insert(at, t);
                let keep = if self.active == i { Some(id) } else { active_id };
                self.active = keep.and_then(|k| self.tabs.iter().position(|x| x.id == k)).unwrap_or(at);
            }
            TabOp::ToggleMute => {
                let t = &mut self.tabs[i];
                t.muted = !t.muted;
                if let Some(wv) = &t.wv {
                    wv.set_muted(t.muted);
                }
            }
            TabOp::CopyLink => {
                let url = self.tabs[i].url.clone();
                let (clean, removed) = match params::clean_navigation(&url) {
                    Some(c) => (c.url, c.removed.len()),
                    None => (url, 0),
                };
                if platform::set_clipboard(&clean) {
                    let msg = if removed > 0 {
                        format!("link copied · {removed} tracking param{} removed", if removed == 1 { "" } else { "s" })
                    } else {
                        "link copied".to_string()
                    };
                    self.toast(msg, None, Tone::Ok);
                }
            }
            TabOp::Close => self.close_tab(i),
            TabOp::CloseOthers => {
                let keep_id = self.tabs[i].id;
                let doomed: Vec<u64> = self.tabs.iter().filter(|t| t.id != keep_id && !t.pinned).map(|t| t.id).collect();
                self.close_ids(&doomed);
            }
            TabOp::CloseRight => {
                let doomed: Vec<u64> = self.tabs[i + 1..].iter().filter(|t| !t.pinned).map(|t| t.id).collect();
                self.close_ids(&doomed);
            }
        }
    }

    fn close_ids(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        let active_id = self.tabs[self.active].id;
        for id in ids {
            if let Some(i) = self.tab_index(*id) {
                let t = self.tabs.remove(i);
                self.remember_closed(&t);
                self.ledger.forget_tab(t.id);
            }
        }
        self.active = self.tabs.iter().position(|t| t.id == active_id).unwrap_or(0);
        self.show_active();
        let n = ids.len();
        self.toast(format!("closed {n} tab{}", if n == 1 { "" } else { "s" }), Some(ToastAct::Reopen), Tone::Plain);
    }

    pub(super) fn remember_closed(&mut self, t: &Tab) {
        if !t.url.is_empty() {
            self.closed.push_front((t.url.clone(), t.title.clone()));
            self.closed.truncate(CLOSED_KEEP);
        }
    }

    pub(super) fn reopen(&mut self) {
        if let Some((url, _)) = self.closed.pop_front() {
            self.new_tab(Some(url));
        }
    }

    // ------------------------------------------------------- engine reports

    pub(super) fn on_audio(&mut self, id: u64, playing: bool) {
        if let Some(i) = self.tab_index(id) {
            self.tabs[i].audio = playing;
        }
    }

    pub(super) fn on_crash(&mut self, id: u64, browser: bool) {
        if browser {
            // the whole engine went down: every live tab must be recreated
            for t in &mut self.tabs {
                if t.wv.is_some() {
                    t.crashed = true;
                    t.wv = None;
                    t.loading = false;
                }
            }
        } else if let Some(i) = self.tab_index(id) {
            self.tabs[i].crashed = true;
            self.tabs[i].loading = false;
        }
    }

    /// Every 30 s: put tabs hidden for 5 minutes to sleep.
    pub(super) fn tick(&mut self) {
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if i == self.active || t.trimmed || t.audio {
                continue;
            }
            if let (Some(wv), Some(since)) = (&t.wv, t.hidden_since) {
                if since.elapsed() >= TRIM_AFTER {
                    wv.suspend();
                    t.trimmed = true;
                }
            }
        }
    }
}
