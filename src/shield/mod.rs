//! The shield: a defense-in-depth telemetry blocker.
//!
//! Layers (each independent, each reports through the event bus):
//! - **Transport** (`platform`): every network request the engine makes is
//!   inspected before it leaves; blocked ones get a fake, well-formed reply.
//! - **Navigation**: top-level navigations are refused or cleaned of
//!   tracking parameters and click-redirect wrappers.
//! - **Runtime** (`runtime.js`): `fetch`, XHR, `sendBeacon`, pixels, script
//!   tags and WebSockets are intercepted inside every frame; analytics
//!   globals are stubbed so pages keep working.
//!
//! Layers never talk to the UI. They `emit` `ShieldEvent`s; the app
//! subscribes a sink and folds events into the `Ledger`.

pub mod lists;
pub mod params;
pub mod rules;
pub mod urlparts;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

pub use rules::{Rules, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Transport,
    Runtime,
    Beacon,
    Params,
    Navigation,
}

impl Layer {
    pub const ALL: [Layer; 5] = [Layer::Transport, Layer::Runtime, Layer::Beacon, Layer::Params, Layer::Navigation];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn letter(self) -> &'static str {
        match self {
            Layer::Transport => "T",
            Layer::Runtime => "R",
            Layer::Beacon => "B",
            Layer::Params => "P",
            Layer::Navigation => "N",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Layer::Transport => "transport · requests dropped",
            Layer::Runtime => "runtime · calls faked",
            Layer::Beacon => "beacons scrambled",
            Layer::Params => "params stripped",
            Layer::Navigation => "navigations refused",
        }
    }

    pub fn from_letter(s: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|l| l.letter() == s)
    }
}

#[derive(Debug, Clone)]
pub struct ShieldEvent {
    pub tab: u64,
    pub layer: Layer,
    pub subject: String,
    pub detail: String,
    pub count: u32,
    /// Raised by the navigation layer for the document about to load; it
    /// survives the per-tab reset that the new document triggers.
    pub sticky: bool,
}

// ---------------------------------------------------------------- event bus

thread_local! {
    static SINK: RefCell<Option<Box<dyn Fn(ShieldEvent)>>> = const { RefCell::new(None) };
}

/// Subscribe the single consumer of shield events (the app).
pub fn subscribe(sink: impl Fn(ShieldEvent) + 'static) {
    SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
}

pub fn emit(ev: ShieldEvent) {
    SINK.with(|s| {
        if let Some(sink) = s.borrow().as_ref() {
            sink(ev)
        }
    });
}

// ------------------------------------------------------------------- shield

/// Hosts the user allowed from a block page this session (also written to
/// filters.txt as @@ rules, which apply from the next start).
static ALLOWED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn allow_host(host: &str) {
    if let Ok(mut a) = ALLOWED.lock() {
        a.push(host.to_string());
    }
}

pub fn is_allowed_host(url: &str) -> bool {
    let Some(p) = urlparts::split(url) else { return false };
    ALLOWED.lock().map_or(false, |a| a.iter().any(|h| urlparts::host_matches(p.host, h)))
}

pub struct Shield {
    pub rules: Rules,
    enabled: Cell<bool>,
}

impl Shield {
    pub fn new(rules: Rules, enabled: bool) -> Self {
        Shield { rules, enabled: Cell::new(enabled) }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.set(on)
    }

    pub fn check(&self, url: &str) -> Verdict {
        if !self.enabled() || is_allowed_host(url) {
            return Verdict::Allow;
        }
        self.rules.check(url)
    }
}

// ------------------------------------------------------------------- ledger

pub struct TabLedger {
    pub counts: [u32; 5],
    pub log: VecDeque<(Layer, String, String)>,
}

const LOG_CAP: usize = 64;

#[derive(Default)]
pub struct Ledger {
    tabs: HashMap<u64, TabLedger>,
    pending: HashMap<u64, Vec<ShieldEvent>>,
    pub session: [u64; 5],
}

impl Ledger {
    pub fn record(&mut self, ev: &ShieldEvent) {
        self.session[ev.layer.index()] += ev.count as u64;
        if ev.sticky {
            self.pending.entry(ev.tab).or_default().push(ev.clone());
        }
        self.tally(ev);
    }

    fn tally(&mut self, ev: &ShieldEvent) {
        let t = self.tabs.entry(ev.tab).or_insert_with(|| TabLedger { counts: [0; 5], log: VecDeque::new() });
        t.counts[ev.layer.index()] += ev.count;
        if t.log.len() == LOG_CAP {
            t.log.pop_back();
        }
        t.log.push_front((ev.layer, ev.subject.clone(), ev.detail.clone()));
    }

    /// A new top-level document starts a fresh per-tab tally.
    pub fn reset_tab(&mut self, tab: u64) {
        self.tabs.remove(&tab);
        for ev in self.pending.remove(&tab).unwrap_or_default() {
            self.tally(&ev);
        }
    }

    pub fn forget_tab(&mut self, tab: u64) {
        self.tabs.remove(&tab);
        self.pending.remove(&tab);
    }

    pub fn tab(&self, tab: u64) -> Option<&TabLedger> {
        self.tabs.get(&tab)
    }

    pub fn tab_total(&self, tab: u64) -> u32 {
        self.tab(tab).map_or(0, |t| t.counts.iter().sum())
    }
}

// ------------------------------------------------------------ runtime layer

const RUNTIME_JS: &str = include_str!("runtime.js");

/// Builds the document-start script injected into every frame.
/// `scroll` is the page-scrollbar config as JSON (see `theme::scroll_json`).
pub fn runtime_script(rules: &Rules, beacon_mode: &str, scroll: &str) -> String {
    let domains = serde_json::to_string(&rules.domain_list()).unwrap_or_else(|_| "[]".into());
    let host_paths = serde_json::to_string(rules.host_path_list()).unwrap_or_else(|_| "[]".into());
    let contains = serde_json::to_string(lists::PATH_CONTAINS).unwrap_or_else(|_| "[]".into());
    let suffix = serde_json::to_string(lists::PATH_SUFFIX).unwrap_or_else(|_| "[]".into());
    let mode = if beacon_mode == "drop" { "drop" } else { "scramble" };
    RUNTIME_JS
        .replace("\"__DOMAINS__\"", &domains)
        .replace("\"__HOST_PATHS__\"", &host_paths)
        .replace("\"__PATH_CONTAINS__\"", &contains)
        .replace("\"__PATH_SUFFIX__\"", &suffix)
        .replace("__BEACON_MODE__", mode)
        .replace("\"__SCROLL__\"", scroll)
        // Windows catches shortcuts natively (AcceleratorKeyPressed); other
        // engines need the in-page listener.
        .replace("__KEY_FALLBACK__", if cfg!(windows) { "false" } else { "true" })
}
