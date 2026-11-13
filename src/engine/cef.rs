//! Bundled Chromium via CEF (tauri-apps/cef-rs).
//!
//! Threads: CEF runs its own UI thread (`multi_threaded_message_loop`), and
//! network callbacks arrive on its IO thread. Nothing here touches app state
//! directly: every report is posted to the app's queue on the Slint thread
//! (`send`). Browser handles are thread-safe in CEF, so the app calls them
//! from its own thread.
//!
//! Shield layers on this engine:
//! - transport: `OnBeforeResourceLoad` strips params and adds Sec-GPC;
//!   `GetResourceHandler` answers blocked requests with a fake reply;
//! - navigation: `OnBeforeBrowse` refuses tracker pages, cleans URLs;
//! - runtime: the renderer runs `runtime.js` the moment each JS context is
//!   created, and bridges `window.ipc.postMessage` back to the app.

use crate::app::{post, DlInfo, DlState, MenuCtx, Msg};
use crate::shield::{self, params, urlparts, Layer, Rules, ShieldEvent, Verdict};
use ::cef::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ------------------------------------------------------------ shared state

static RULES: OnceLock<Rules> = OnceLock::new();
static SHIELD_ON: AtomicBool = AtomicBool::new(true);
static HTTPS: AtomicBool = AtomicBool::new(false);
/// param-strip redirects per tab since the last finished load (loop guard)
static STRIPS: Mutex<Option<HashMap<u64, u8>>> = Mutex::new(None);

/// Sites where the user paused the shield: requests are counted, not blocked.
static PAUSED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Hosts whose certificate error the user chose to override once (`!url`).
static CERT_ONCE: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Top-level site per tab, for per-site pause on subresource requests.
static TAB_SITE: Mutex<Option<HashMap<u64, String>>> = Mutex::new(None);
/// Pending permission prompts and live downloads, answered from the app.
static PROMPTS: Mutex<Option<HashMap<u64, Prompt>>> = Mutex::new(None);
static DOWNLOADS: Mutex<Option<HashMap<u32, DlCallback>>> = Mutex::new(None);
static NEXT_PROMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

enum Prompt {
    Media(MediaAccessCallback, u32),
    Generic(PermissionPromptCallback),
}
unsafe impl Send for Prompt {}
struct DlCallback(DownloadItemCallback);
unsafe impl Send for DlCallback {}

pub fn set_paused(sites: &[String]) {
    if let Ok(mut p) = PAUSED.lock() {
        *p = sites.to_vec();
    }
}

fn paused_for(tab: u64) -> bool {
    let site = TAB_SITE.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&tab).cloned()));
    match site {
        Some(s) => PAUSED.lock().map_or(false, |p| p.iter().any(|x| urlparts::host_matches(&s, x))),
        None => false,
    }
}

pub fn allow_cert_once(host: &str) {
    if let Ok(mut c) = CERT_ONCE.lock() {
        c.push(host.to_string());
    }
}

/// Answer a permission prompt: allow = grant, once = do not remember (the
/// app remembers; the engine is told the same either way).
pub fn answer_prompt(id: u64, allow: bool) {
    let p = PROMPTS.lock().ok().and_then(|mut m| m.get_or_insert_with(HashMap::new).remove(&id));
    on_ui(move || match p {
        Some(Prompt::Media(cb, wanted)) => {
            if allow {
                cb.cont(wanted)
            } else {
                cb.cancel()
            }
        }
        Some(Prompt::Generic(cb)) => cb.cont(if allow { PermissionRequestResult::ACCEPT } else { PermissionRequestResult::DENY }),
        None => {}
    });
}

pub fn download_action(id: u32, action: &'static str) {
    let cb = DOWNLOADS.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&id).map(|c| DlCallback(c.0.clone()))));
    if let Some(cb) = cb {
        on_ui(move || match action {
            "pause" => cb.0.pause(),
            "resume" => cb.0.resume(),
            _ => cb.0.cancel(),
        });
    }
}

/// Erase cookies for one site (site info › clear site data).
pub fn clear_site(url: &str) {
    let url = url.to_string();
    on_ui(move || {
        if let Some(cm) = cookie_manager_get_global_manager(None) {
            cm.delete_cookies(Some(&s(&url)), None, None);
        }
    });
}

pub fn set_shield(on: bool) {
    SHIELD_ON.store(on, Ordering::Relaxed);
}

pub fn set_https(on: bool) {
    HTTPS.store(on, Ordering::Relaxed);
}

fn rules() -> &'static Rules {
    RULES.get_or_init(|| {
        let mut r = Rules::builtin();
        r.load_file(&crate::config::dir().join("filters.txt"));
        r
    })
}

fn check(url: &str) -> Verdict {
    if SHIELD_ON.load(Ordering::Relaxed) {
        if crate::shield::is_allowed_host(url) {
            return Verdict::Allow;
        }
        rules().check(url)
    } else {
        Verdict::Allow
    }
}

/// Post to the app from any CEF thread.
fn send(m: Msg) {
    let _ = slint::invoke_from_event_loop(move || post(m));
}

fn emit(tab: u64, layer: Layer, subject: String, detail: String, count: u32, sticky: bool) {
    send(Msg::Shield(ShieldEvent { tab, layer, subject, detail, count, sticky }));
}

fn s(v: &str) -> CefString {
    CefString::from(v)
}

type Job = Box<dyn FnOnce() + Send>;

wrap_task! {
    struct UiTask {
        job: Arc<Mutex<Option<Job>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(job) = self.job.lock().ok().and_then(|mut j| j.take()) {
                job();
            }
        }
    }
}

/// Most browser-host calls are only valid on CEF's UI thread: run there.
fn on_ui(job: impl FnOnce() + Send + 'static) {
    let mut task = UiTask::new(Arc::new(Mutex::new(Some(Box::new(job)))));
    post_task(ThreadId::UI, Some(&mut task));
}

// ------------------------------------------------------------- startup

/// CEF starts every helper process (renderer, GPU, network) by running this
/// same exe. Call first thing in `main`: returns the exit code when this is
/// a helper, `None` in the browser process.
/// Whether the engine files sit next to the exe (libcef.dll is delay-loaded,
/// so nothing CEF may be called without them).
pub fn available() -> bool {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("libcef.dll").exists())).unwrap_or(false)
}

pub fn run_subprocess() -> Option<i32> {
    if !available() {
        return None;
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let mut app = BrxApp::new();
    let code = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    (code >= 0).then_some(code)
}

/// Start the engine in the browser process. `cache` = profile folder;
/// `None` keeps everything in memory (ephemeral windows).
static STARTED: AtomicBool = AtomicBool::new(false);
/// CEF drops browsers created before its context is up; queue them.
static CONTEXT_READY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Vec<Job>> = Mutex::new(Vec::new());

fn when_ready(job: Job) {
    let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if CONTEXT_READY.load(Ordering::Acquire) {
        drop(q);
        job();
    } else {
        q.push(job);
    }
}

wrap_browser_process_handler! {
    struct BrxBrowserProcess;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            let jobs = {
                let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                CONTEXT_READY.store(true, Ordering::Release);
                std::mem::take(&mut *q)
            };
            for job in jobs {
                job();
            }
        }
    }
}

pub fn started() -> bool {
    STARTED.load(Ordering::Relaxed)
}

/// Start once; later calls report whether the first one worked.
pub fn ensure_started(ephemeral: bool) -> bool {
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(|| {
        if !available() {
            return false;
        }
        let root = if ephemeral {
            std::env::temp_dir().join(format!("brxswer-eph-{}", std::process::id()))
        } else {
            crate::config::dir().join("chromium")
        };
        let cache = (!ephemeral).then(|| root.join("profile"));
        let ok = start(&root, cache.as_deref());
        STARTED.store(ok, Ordering::Relaxed);
        ok
    })
}

fn start(root: &std::path::Path, cache: Option<&std::path::Path>) -> bool {
    // the version handshake must precede any other CEF call in this process
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let mut app = BrxApp::new();
    let settings = Settings {
        no_sandbox: 1,
        multi_threaded_message_loop: 1,
        root_cache_path: s(&root.to_string_lossy()),
        cache_path: cache.map_or_else(|| s(""), |c| s(&c.to_string_lossy())),
        persist_session_cookies: 1,
        log_severity: LogSeverity::DISABLE,
        // one language pack ships; every user sends the same Accept-Language
        locale: s("en-US"),
        ..Default::default()
    };
    initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) == 1
}

pub fn stop() {
    shutdown();
}

wrap_app! {
    struct BrxApp;

    impl App {
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            let browser = process_type.map_or(true, |p| p.to_string().is_empty());
            let Some(cl) = command_line else { return };
            if browser {
                // Chromium's own phone-home traffic
                for sw in [
                    "no-pings",
                    "disable-background-networking",
                    "disable-component-update",
                    "disable-domain-reliability",
                    "disable-breakpad",
                    "disable-client-side-phishing-detection",
                    "disable-sync",
                    "metrics-recording-only",
                    "no-default-browser-check",
                    "disable-default-apps",
                ] {
                    cl.append_switch(Some(&s(sw)));
                }
                cl.append_switch_with_value(Some(&s("disable-features")), Some(&s("OptimizationHints,MediaRouter,InterestFeedContentSuggestions,Translate,AutofillServerCommunication")));
                let autoplay = crate::config::Config::load().autoplay;
                cl.append_switch_with_value(
                    Some(&s("autoplay-policy")),
                    Some(&s(if autoplay == "allow" { "no-user-gesture-required" } else { "document-user-activation-required" })),
                );
            }
        }

        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(BrxRender::new())
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(BrxBrowserProcess::new())
        }
    }
}

// ------------------------------------------------------------- renderer

fn runtime_script() -> &'static str {
    static SCRIPT: OnceLock<String> = OnceLock::new();
    SCRIPT.get_or_init(|| {
        let cfg = crate::config::Config::load();
        shield::runtime_script(rules(), &cfg.beacons, &crate::theme::scroll_json(&cfg.scrollbars, &cfg.theme))
    })
}

wrap_render_process_handler! {
    struct BrxRender;

    impl RenderProcessHandler {
        fn on_context_created(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            let (Some(frame), Some(context)) = (frame, context) else { return };
            // window.ipc.postMessage(string) -> browser process
            if let Some(global) = context.global() {
                let mut handler = IpcBridge::new(Arc::new(Mutex::new(SendFrame(frame.clone()))));
                if let (Some(mut obj), Some(mut func)) = (
                    v8_value_create_object(None, None),
                    v8_value_create_function(Some(&s("postMessage")), Some(&mut handler)),
                ) {
                    obj.set_value_bykey(Some(&s("postMessage")), Some(&mut func), Default::default());
                    global.set_value_bykey(Some(&s("ipc")), Some(&mut obj), Default::default());
                }
            }
            // the runtime shield layer, before any page script runs; sites
            // the user paused get only the scrollbar part
            let paused = crate::config::Config::load().paused;
            let pre = format!("window.__brxPaused = {};", serde_json::to_string(&paused).unwrap_or_else(|_| "[]".into()));
            let mut retval = None;
            let mut exception = None;
            context.eval(Some(&s(&format!("{pre}\n{}", runtime_script()))), Some(&s("brx://shield/runtime.js")), 0, Some(&mut retval), Some(&mut exception));
        }
    }
}

struct SendFrame(Frame);
unsafe impl Send for SendFrame {}

wrap_v8_handler! {
    struct IpcBridge {
        frame: Arc<Mutex<SendFrame>>,
    }

    impl V8Handler {
        fn execute(&self, _name: Option<&CefString>, _object: Option<&mut V8Value>, arguments: Option<&[Option<V8Value>]>, _retval: Option<&mut Option<V8Value>>, _exception: Option<&mut CefString>) -> i32 {
            let text = arguments
                .and_then(|a| a.first().cloned().flatten())
                .filter(|v| v.is_string() != 0)
                .map(|v| CefString::from(&v.string_value()).to_string());
            if let (Some(text), Some(mut msg)) = (text, process_message_create(Some(&s("brx-ipc")))) {
                if let Some(list) = msg.argument_list() {
                    list.set_string(0, Some(&s(&text)));
                }
                if let Ok(f) = self.frame.lock() {
                    f.0.send_process_message(ProcessId::BROWSER, Some(&mut msg));
                }
            }
            1
        }
    }
}

// ------------------------------------------------------------- browser

#[derive(Clone, Copy, Default)]
pub struct State {
    pub can_back: bool,
    pub can_forward: bool,
}

#[derive(Default)]
struct Shared {
    browser: Option<Browser>,
    /// physical px, relative to the parent window
    rect: (i32, i32, i32, i32),
    visible: bool,
    muted: bool,
    state: State,
}

struct Cell(Shared);
unsafe impl Send for Cell {}

type SharedRef = Arc<Mutex<Cell>>;

/// The app's handle to one CEF browser. The browser itself is created
/// asynchronously; calls made before it exists are applied when it does.
pub struct Handle {
    parent: isize,
    shared: SharedRef,
}

fn place(b: &Browser, rect: (i32, i32, i32, i32), visible: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW};
    if let Some(host) = b.host() {
        let h = HWND(host.window_handle().0 as *mut _);
        unsafe {
            let _ = SetWindowPos(h, None, rect.0, rect.1, rect.2.max(1), rect.3.max(1), SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = ShowWindow(h, if visible { SW_SHOW } else { SW_HIDE });
        }
        host.was_resized();
        host.was_hidden(!visible as i32);
    }
}

impl Handle {
    pub fn create(tab: u64, parent: isize, rect: (i32, i32, i32, i32), url: &str, ephemeral: bool) -> Handle {
        let shared: SharedRef = Arc::new(Mutex::new(Cell(Shared { rect, visible: true, ..Default::default() })));
        let job_shared = shared.clone();
        let url = url.to_string();
        when_ready(Box::new(move || {
            // geometry may have changed while the engine was starting
            let rect = job_shared.lock().map_or(rect, |g| g.0.rect);
            // Alloy: a bare page for embedding (the Chrome style expects to
            // own a Chrome window with its own toolbar)
            let window_info = WindowInfo { runtime_style: RuntimeStyle::ALLOY, ..Default::default() }.set_as_child(
                sys::HWND(parent as *mut sys::HWND__),
                &::cef::Rect { x: rect.0, y: rect.1, width: rect.2, height: rect.3 },
            );
            let mut client = BrxClient::new(tab, job_shared);
            let settings = BrowserSettings::default();
            // ephemeral: a context with no cache path lives only in memory
            let mut ctx = if ephemeral { request_context_create_context(Some(&RequestContextSettings::default()), None) } else { None };
            browser_host_create_browser(Some(&window_info), Some(&mut client), Some(&s(&url)), Some(&settings), None, ctx.as_mut());
        }));
        Handle { parent, shared }
    }

    /// Run `f` with the browser on CEF's UI thread (skipped if it does not
    /// exist yet; state that matters is applied in `on_after_created`).
    fn with(&self, f: impl FnOnce(&Browser) + Send + 'static) {
        let shared = self.shared.clone();
        on_ui(move || {
            // Never call into CEF while holding the lock: CEF may call our
            // handlers back synchronously, and they take the same lock.
            let b = shared.lock().ok().and_then(|g| g.0.browser.clone());
            if let Some(b) = b {
                f(&b);
            }
        });
    }

    pub fn state(&self) -> State {
        self.shared.lock().map_or(State::default(), |g| g.0.state)
    }

    pub fn load_url(&self, url: &str) {
        let url = url.to_string();
        self.with(move |b| {
            if let Some(f) = b.main_frame() {
                f.load_url(Some(&s(&url)));
            }
        });
    }

    pub fn load_html(&self, html: &str) {
        self.load_url(&format!("data:text/html;charset=utf-8;base64,{}", base64(html.as_bytes())));
    }

    pub fn reload(&self) {
        self.with(|b| b.reload());
    }

    pub fn find(&self, text: &str, forward: bool, case: bool, next: bool) {
        let text = text.to_string();
        self.with(move |b| {
            if let Some(h) = b.host() {
                if text.is_empty() {
                    h.stop_finding(1);
                } else {
                    h.find(Some(&s(&text)), forward as i32, case as i32, next as i32);
                }
            }
        });
    }

    pub fn stop_find(&self, keep_selection: bool) {
        self.with(move |b| {
            if let Some(h) = b.host() {
                h.stop_finding(!keep_selection as i32);
            }
        });
    }

    /// `percent` 100 = normal. Chromium zoom levels are log base 1.2.
    pub fn set_zoom(&self, percent: u32) {
        let level = (percent as f64 / 100.0).ln() / 1.2f64.ln();
        self.with(move |b| {
            if let Some(h) = b.host() {
                h.set_zoom_level(level);
            }
        });
    }

    pub fn devtools(&self) {
        self.with(|b| {
            if let Some(h) = b.host() {
                let info = WindowInfo::default();
                #[cfg(windows)]
                let info = info.set_as_popup(sys::HWND::default(), "brxswer devtools");
                h.show_dev_tools(Some(&info), None, Some(&BrowserSettings::default()), None);
            }
        });
    }

    pub fn edit(&self, what: &'static str) {
        self.with(move |b| {
            if let Some(f) = b.main_frame() {
                match what {
                    "cut" => f.cut(),
                    "copy" => f.copy(),
                    "paste" => f.paste(),
                    "selectall" => f.select_all(),
                    "source" => f.view_source(),
                    _ => {}
                }
            }
        });
    }

    pub fn run_script(&self, js: &str) {
        let js = js.to_string();
        self.with(move |b| {
            if let Some(f) = b.main_frame() {
                f.execute_java_script(Some(&s(&js)), Some(&s("brx://scroll")), 0);
            }
        });
    }

    pub fn stop(&self) {
        self.with(|b| b.stop_load());
    }

    pub fn go_back(&self) {
        self.with(|b| b.go_back());
    }

    pub fn go_forward(&self) {
        self.with(|b| b.go_forward());
    }

    pub fn set_visible(&self, on: bool) {
        let shared = self.shared.clone();
        on_ui(move || {
            let todo = shared.lock().ok().and_then(|mut g| {
                g.0.visible = on;
                g.0.browser.clone().map(|b| (b, g.0.rect))
            });
            if let Some((b, rect)) = todo {
                place(&b, rect, on);
            }
        });
    }

    pub fn set_bounds(&self, rect: wry::Rect, scale: f64) {
        let p = rect.position.to_physical::<i32>(scale);
        let z = rect.size.to_physical::<i32>(scale);
        let shared = self.shared.clone();
        on_ui(move || {
            let todo = shared.lock().ok().and_then(|mut g| {
                g.0.rect = (p.x, p.y, z.width, z.height);
                g.0.browser.clone().map(|b| (b, g.0.rect, g.0.visible))
            });
            if let Some((b, rect, visible)) = todo {
                place(&b, rect, visible);
            }
        });
    }

    pub fn focus(&self) {
        self.with(|b| {
            if let Some(h) = b.host() {
                h.set_focus(1);
            }
        });
    }

    pub fn focus_parent(&self) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
        self.with(move |b| {
            if let Some(h) = b.host() {
                h.set_focus(0);
            }
        });
        unsafe {
            let _ = SetFocus(Some(HWND(self.parent as *mut _)));
        }
    }

    pub fn set_background(&self, background: bool) {
        self.with(move |b| {
            if let Some(h) = b.host() {
                h.was_hidden(background as i32);
            }
        });
    }

    pub fn set_muted(&self, muted: bool) {
        let shared = self.shared.clone();
        on_ui(move || {
            let b = shared.lock().ok().and_then(|mut g| {
                g.0.muted = muted;
                g.0.browser.clone()
            });
            if let Some(h) = b.and_then(|b| b.host()) {
                h.set_audio_muted(muted as i32);
            }
        });
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.with(|b| {
            if let Some(h) = b.host() {
                h.close_browser(1);
            }
        });
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ------------------------------------------------------------- handlers

wrap_client! {
    struct BrxClient {
        tab: u64,
        shared: SharedRef,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(BrxDisplay::new(self.tab))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(BrxLife::new(self.tab, self.shared.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(BrxLoad::new(self.tab, self.shared.clone()))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(BrxRequest::new(self.tab))
        }
        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            Some(BrxKeys::new(self.tab))
        }
        fn focus_handler(&self) -> Option<FocusHandler> {
            Some(BrxFocus::new(self.tab))
        }
        fn find_handler(&self) -> Option<FindHandler> {
            Some(BrxFind::new(self.tab))
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(BrxDownloads::new(self.tab))
        }
        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(BrxPermissions::new(self.tab))
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(BrxMenu::new(self.tab))
        }
        fn on_process_message_received(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _source: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let Some(m) = message else { return 0 };
            if CefString::from(&m.name()).to_string() != "brx-ipc" {
                return 0;
            }
            if let Some(list) = m.argument_list() {
                send(Msg::Ipc(self.tab, CefString::from(&list.string(0)).to_string()));
            }
            1
        }
    }
}

wrap_display_handler! {
    struct BrxDisplay {
        tab: u64,
    }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            send(Msg::Title(self.tab, title.map(|t| t.to_string()).unwrap_or_default()));
        }
        fn on_loading_progress_change(&self, _browser: Option<&mut Browser>, progress: f64) {
            send(Msg::Progress(self.tab, progress as f32));
        }
        // the hovered link's destination
        fn on_status_message(&self, _browser: Option<&mut Browser>, value: Option<&CefString>) {
            send(Msg::Status(self.tab, value.map(|v| v.to_string()).unwrap_or_default()));
        }
        fn on_fullscreen_mode_change(&self, _browser: Option<&mut Browser>, fullscreen: i32) {
            send(Msg::Fullscreen(fullscreen != 0));
        }
    }
}

wrap_life_span_handler! {
    struct BrxLife {
        tab: u64,
        shared: SharedRef,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(b) = browser else { return };
            let state = self.shared.lock().ok().map(|mut g| {
                g.0.browser = Some(b.clone());
                (g.0.rect, g.0.visible, g.0.muted)
            });
            if let Some((rect, visible, muted)) = state {
                place(b, rect, visible);
                if muted {
                    if let Some(h) = b.host() {
                        h.set_audio_muted(1);
                    }
                }
            }
            send(Msg::Geometry);
        }
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            // window.open and target=_blank become tabs
            if let Some(u) = target_url {
                send(Msg::OpenTab(u.to_string()));
            }
            1
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if let Ok(mut g) = self.shared.lock() {
                g.0.browser = None;
            }
        }
    }
}

wrap_load_handler! {
    struct BrxLoad {
        tab: u64,
        shared: SharedRef,
    }

    impl LoadHandler {
        fn on_loading_state_change(&self, browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            if let Ok(mut g) = self.shared.lock() {
                g.0.state = State { can_back: can_go_back != 0, can_forward: can_go_forward != 0 };
            }
            if is_loading == 0 {
                let url = browser
                    .and_then(|b| b.main_frame())
                    .map(|f| CefString::from(&f.url()).to_string())
                    .unwrap_or_default();
                if let Ok(mut m) = STRIPS.lock() {
                    m.get_or_insert_with(HashMap::new).remove(&self.tab);
                }
                send(Msg::LoadFinished(self.tab, url));
            }
        }
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, failed_url: Option<&CefString>) {
            let Some(f) = frame else { return };
            // ERR_ABORTED: we (or the user) cancelled it; not an error to show
            if f.is_main() == 0 || error_code == Errorcode::ABORTED {
                return;
            }
            let code: sys::cef_errorcode_t = error_code.into();
            send(Msg::LoadError(
                self.tab,
                failed_url.map(|u| u.to_string()).unwrap_or_default(),
                code as i32,
                error_text.map(|t| t.to_string()).unwrap_or_default(),
            ));
        }
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            if let Some(f) = frame {
                if f.is_main() != 0 {
                    send(Msg::LoadStarted(self.tab, CefString::from(&f.url()).to_string()));
                }
            }
        }
    }
}

wrap_keyboard_handler! {
    struct BrxKeys {
        tab: u64,
    }

    impl KeyboardHandler {
        fn on_pre_key_event(&self, _browser: Option<&mut Browser>, event: Option<&KeyEvent>, _os_event: Option<&mut sys::MSG>, _is_keyboard_shortcut: Option<&mut i32>) -> i32 {
            let Some(e) = event else { return 0 };
            if e.type_ != KeyEventType::RAWKEYDOWN {
                return 0;
            }
            let m = e.modifiers;
            match crate::platform::combo(e.windows_key_code as u32, m & 4 != 0, m & 8 != 0, m & 2 != 0) {
                Some(c) => {
                    send(Msg::Key(c));
                    1
                }
                None => 0,
            }
        }
    }
}

wrap_focus_handler! {
    struct BrxFocus {
        tab: u64,
    }

    impl FocusHandler {
        fn on_got_focus(&self, _browser: Option<&mut Browser>) {
            send(Msg::WebFocus);
        }
    }
}

wrap_request_handler! {
    struct BrxRequest {
        tab: u64,
    }

    impl RequestHandler {
        // navigation layer
        fn on_before_browse(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, request: Option<&mut Request>, _user_gesture: i32, _is_redirect: i32) -> i32 {
            let (Some(frame), Some(req)) = (frame, request) else { return 0 };
            if frame.is_main() == 0 {
                return 0;
            }
            let url = CefString::from(&req.url()).to_string();
            if let (Some(p), Ok(mut m)) = (urlparts::split(&url), TAB_SITE.lock()) {
                m.get_or_insert_with(HashMap::new).insert(self.tab, p.host.to_string());
            }
            navigation(self.tab, &url) as i32
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: i32,
            _is_download: i32,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut i32>,
        ) -> Option<ResourceRequestHandler> {
            Some(BrxTransport::new(self.tab))
        }

        fn on_certificate_error(&self, _browser: Option<&mut Browser>, cert_error: Errorcode, request_url: Option<&CefString>, _ssl_info: Option<&mut Sslinfo>, callback: Option<&mut Callback>) -> i32 {
            let url = request_url.map(|u| u.to_string()).unwrap_or_default();
            let host = urlparts::split(&url).map_or(String::new(), |p| p.host.to_string());
            let once = CERT_ONCE.lock().ok().and_then(|mut c| c.iter().position(|h| *h == host).map(|i| c.remove(i))).is_some();
            if once {
                if let Some(cb) = callback {
                    cb.cont();
                }
                return 1;
            }
            let code: sys::cef_errorcode_t = cert_error.into();
            send(Msg::CertError(self.tab, url, code as i32));
            0
        }

        fn on_render_process_terminated(&self, _browser: Option<&mut Browser>, _status: TerminationStatus, _error_code: i32, _error_string: Option<&CefString>) {
            send(Msg::Crashed(self.tab, false));
        }
    }
}

/// Returns true to cancel the navigation.
fn navigation(tab: u64, url: &str) -> bool {
    if HTTPS.load(Ordering::Relaxed) {
        if let Some(rest) = url.strip_prefix("http://") {
            let host = urlparts::split(url).map_or("", |p| p.host);
            let local = host == "localhost" || host.ends_with(".localhost") || host.ends_with(".test") || host.parse::<std::net::IpAddr>().is_ok();
            if !local {
                send(Msg::Redirect(tab, format!("https://{rest}")));
                return true;
            }
        }
    }
    if !SHIELD_ON.load(Ordering::Relaxed) || !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    if paused_for(tab) {
        return false;
    }
    if let Verdict::Block(rule) = check(url) {
        let host = urlparts::split(url).map_or(String::new(), |p| p.host.to_string());
        emit(tab, Layer::Navigation, host, " page refused".into(), 1, true);
        send(Msg::BlockedNav(tab, url.to_string(), rule));
        return true;
    }
    let Ok(mut strips) = STRIPS.lock() else { return false };
    let n = strips.get_or_insert_with(HashMap::new).entry(tab).or_insert(0);
    if *n < 3 {
        if let Some(c) = params::clean_navigation(url) {
            *n += 1;
            emit(tab, Layer::Params, c.removed.join(", "), " stripped".into(), c.removed.len() as u32, true);
            send(Msg::Redirect(tab, c.url));
            return true;
        }
    }
    false
}

fn kind_name(t: ResourceType) -> &'static str {
    match t {
        ResourceType::SCRIPT => "script",
        ResourceType::IMAGE | ResourceType::FAVICON => "pixel",
        ResourceType::STYLESHEET => "css",
        ResourceType::XHR => "xhr",
        ResourceType::PING => "ping",
        ResourceType::SUB_FRAME => "frame",
        ResourceType::MEDIA => "media",
        ResourceType::FONT_RESOURCE => "font",
        _ => "other",
    }
}

wrap_resource_request_handler! {
    struct BrxTransport {
        tab: u64,
    }

    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue {
            let Some(req) = request else { return ReturnValue::CONTINUE };
            let url = CefString::from(&req.url()).to_string();
            if !matches!(check(&url), Verdict::Allow) {
                // answered by resource_handler with a fake reply
                return ReturnValue::CONTINUE;
            }
            if SHIELD_ON.load(Ordering::Relaxed) && !paused_for(self.tab) {
                req.set_header_by_name(Some(&s("Sec-GPC")), Some(&s("1")), 1);
                if req.resource_type() != ResourceType::MAIN_FRAME {
                    if let Some(c) = params::strip_generic(&url) {
                        req.set_url(Some(&s(&c.url)));
                        emit(self.tab, Layer::Params, c.removed.join(", "), format!(" stripped · {}", kind_name(req.resource_type())), c.removed.len() as u32, false);
                    }
                }
            }
            ReturnValue::CONTINUE
        }

        fn resource_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>) -> Option<ResourceHandler> {
            let req = request?;
            let url = CefString::from(&req.url()).to_string();
            if req.resource_type() == ResourceType::MAIN_FRAME {
                return None;
            }
            match check(&url) {
                Verdict::Allow => None,
                Verdict::Block(_) => {
                    let t = req.resource_type();
                    let p = urlparts::split(&url);
                    let paused = paused_for(self.tab);
                    emit(
                        self.tab,
                        Layer::Transport,
                        p.map_or_else(String::new, |p| p.host.to_string()),
                        format!("{} · {}{}", p.map_or("", |p| p.path), kind_name(t), if paused { " (seen)" } else { "" }),
                        1,
                        false,
                    );
                    if paused {
                        return None;
                    }
                    let origin = CefString::from(&req.header_by_name(Some(&s("Origin")))).to_string();
                    Some(FakeReply::new(Arc::new(Mutex::new(Fake::for_kind(t, origin)))))
                }
            }
        }
    }
}

// ---------------------------------------------------------- M3 handlers

wrap_find_handler! {
    struct BrxFind {
        tab: u64,
    }

    impl FindHandler {
        fn on_find_result(&self, _browser: Option<&mut Browser>, _identifier: i32, count: i32, _selection_rect: Option<&::cef::Rect>, active_match_ordinal: i32, _final_update: i32) {
            send(Msg::FindResult(self.tab, count, active_match_ordinal));
        }
    }
}

fn downloads_dir() -> std::path::PathBuf {
    dirs::download_dir().unwrap_or_else(|| crate::config::dir().join("downloads"))
}

/// A free file name in the downloads folder ("name (2).ext" if taken).
fn free_name(name: &str) -> std::path::PathBuf {
    let dir = downloads_dir();
    let name = if name.trim().is_empty() { "download" } else { name };
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((a, b)) => (a.to_string(), format!(".{b}")),
        None => (name.to_string(), String::new()),
    };
    (2..1000).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap_or(p)
}

wrap_download_handler! {
    struct BrxDownloads {
        tab: u64,
    }

    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> i32 {
            1
        }

        fn on_before_download(&self, _browser: Option<&mut Browser>, _download_item: Option<&mut DownloadItem>, suggested_name: Option<&CefString>, callback: Option<&mut BeforeDownloadCallback>) -> i32 {
            let path = free_name(&suggested_name.map(|n| n.to_string()).unwrap_or_default());
            if let Some(cb) = callback {
                cb.cont(Some(&s(&path.to_string_lossy())), 0);
            }
            1
        }

        fn on_download_updated(&self, _browser: Option<&mut Browser>, download_item: Option<&mut DownloadItem>, callback: Option<&mut DownloadItemCallback>) {
            let Some(d) = download_item else { return };
            if d.is_valid() == 0 {
                return;
            }
            let id = d.id();
            if let (Some(cb), Ok(mut m)) = (callback, DOWNLOADS.lock()) {
                m.get_or_insert_with(HashMap::new).insert(id, DlCallback(cb.clone()));
            }
            let path = CefString::from(&d.full_path()).to_string();
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| CefString::from(&d.suggested_file_name()).to_string());
            let state = if d.is_complete() != 0 {
                DlState::Done
            } else if d.is_canceled() != 0 {
                DlState::Canceled
            } else if d.is_interrupted() != 0 {
                DlState::Failed
            } else {
                DlState::Running
            };
            if state != DlState::Running {
                if let Ok(mut m) = DOWNLOADS.lock() {
                    m.get_or_insert_with(HashMap::new).remove(&id);
                }
            }
            send(Msg::Download(DlInfo {
                id,
                name,
                path,
                received: d.received_bytes(),
                total: d.total_bytes(),
                speed: d.current_speed(),
                state,
            }));
        }
    }
}

const PERM_CAMERA: u32 = 4;
const PERM_GEO: u32 = 256;
const PERM_MIC: u32 = 4096;
const PERM_NOTIFY: u32 = 32768;
const MEDIA_AUDIO: u32 = 1;
const MEDIA_VIDEO: u32 = 2;

wrap_permission_handler! {
    struct BrxPermissions {
        tab: u64,
    }

    impl PermissionHandler {
        // getUserMedia: camera / microphone
        fn on_request_media_access_permission(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, requesting_origin: Option<&CefString>, requested_permissions: u32, callback: Option<&mut MediaAccessCallback>) -> i32 {
            let Some(cb) = callback else { return 0 };
            let wanted = requested_permissions & (MEDIA_AUDIO | MEDIA_VIDEO);
            if wanted == 0 {
                cb.cancel();
                return 1;
            }
            let kinds = (if wanted & MEDIA_VIDEO != 0 { PERM_CAMERA } else { 0 }) | (if wanted & MEDIA_AUDIO != 0 { PERM_MIC } else { 0 });
            let id = NEXT_PROMPT.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut m) = PROMPTS.lock() {
                m.get_or_insert_with(HashMap::new).insert(id, Prompt::Media(cb.clone(), wanted));
            }
            send(Msg::PermAsk(self.tab, id, requesting_origin.map(|o| o.to_string()).unwrap_or_default(), kinds));
            1
        }

        fn on_show_permission_prompt(&self, _browser: Option<&mut Browser>, _prompt_id: u64, requesting_origin: Option<&CefString>, requested_permissions: u32, callback: Option<&mut PermissionPromptCallback>) -> i32 {
            let Some(cb) = callback else { return 0 };
            let origin = requesting_origin.map(|o| o.to_string()).unwrap_or_default();
            // notifications are refused quietly; the address bar shows it
            if requested_permissions & PERM_NOTIFY != 0 && requested_permissions & !PERM_NOTIFY == 0 {
                cb.cont(PermissionRequestResult::DENY);
                send(Msg::PermQuiet(self.tab, origin));
                return 1;
            }
            let kinds = requested_permissions & (PERM_CAMERA | PERM_MIC | PERM_GEO);
            if kinds == 0 {
                cb.cont(PermissionRequestResult::DENY);
                return 1;
            }
            let id = NEXT_PROMPT.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut m) = PROMPTS.lock() {
                m.get_or_insert_with(HashMap::new).insert(id, Prompt::Generic(cb.clone()));
            }
            send(Msg::PermAsk(self.tab, id, origin, kinds));
            1
        }
    }
}

wrap_context_menu_handler! {
    struct BrxMenu {
        tab: u64,
    }

    impl ContextMenuHandler {
        // draw our own menu instead of Chromium's
        fn on_before_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            if let Some(m) = model {
                m.clear();
            }
            let Some(p) = params else { return };
            send(Msg::PageMenu(self.tab, MenuCtx {
                x: p.xcoord(),
                y: p.ycoord(),
                link: CefString::from(&p.unfiltered_link_url()).to_string(),
                selection: CefString::from(&p.selection_text()).to_string(),
                editable: p.is_editable() != 0,
                page: CefString::from(&p.page_url()).to_string(),
            }));
        }
    }
}

// --------------------------------------------------------- fake replies

const GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
    0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02,
    0x01, 0x44, 0x00, 0x3b,
];

/// A well-formed, empty reply matching what the page asked for, so tracker
/// loaders fire `onload` and stop retrying.
struct Fake {
    status: i32,
    mime: &'static str,
    body: &'static [u8],
    origin: String,
    sent: usize,
}

impl Fake {
    fn for_kind(t: ResourceType, origin: String) -> Fake {
        let (status, mime, body): (i32, &str, &[u8]) = match t {
            ResourceType::SCRIPT => (200, "text/javascript", b""),
            ResourceType::STYLESHEET => (200, "text/css", b""),
            ResourceType::SUB_FRAME => (200, "text/html", b""),
            ResourceType::IMAGE | ResourceType::FAVICON => (200, "image/gif", GIF),
            _ => (204, "text/plain", b""),
        };
        Fake { status, mime, body, origin, sent: 0 }
    }
}

wrap_resource_handler! {
    struct FakeReply {
        fake: Arc<Mutex<Fake>>,
    }

    impl ResourceHandler {
        fn open(&self, _request: Option<&mut Request>, handle_request: Option<&mut i32>, _callback: Option<&mut Callback>) -> i32 {
            if let Some(h) = handle_request {
                *h = 1;
            }
            1
        }

        fn response_headers(&self, response: Option<&mut Response>, response_length: Option<&mut i64>, _redirect_url: Option<&mut CefString>) {
            let Ok(f) = self.fake.lock() else { return };
            if let Some(r) = response {
                r.set_status(f.status);
                r.set_status_text(Some(&s(if f.status == 204 { "No Content" } else { "OK" })));
                r.set_mime_type(Some(&s(f.mime)));
                r.set_header_by_name(Some(&s("Cache-Control")), Some(&s("no-store")), 1);
                if f.origin.is_empty() || f.origin == "null" {
                    r.set_header_by_name(Some(&s("Access-Control-Allow-Origin")), Some(&s("*")), 1);
                } else {
                    r.set_header_by_name(Some(&s("Access-Control-Allow-Origin")), Some(&s(&f.origin)), 1);
                    r.set_header_by_name(Some(&s("Access-Control-Allow-Credentials")), Some(&s("true")), 1);
                }
            }
            if let Some(len) = response_length {
                *len = f.body.len() as i64;
            }
        }

        fn read(&self, data_out: *mut u8, bytes_to_read: i32, bytes_read: Option<&mut i32>, _callback: Option<&mut ResourceReadCallback>) -> i32 {
            let Ok(mut f) = self.fake.lock() else { return 0 };
            let left = &f.body[f.sent.min(f.body.len())..];
            let n = left.len().min(bytes_to_read.max(0) as usize);
            if n == 0 {
                if let Some(b) = bytes_read {
                    *b = 0;
                }
                return 0;
            }
            unsafe { std::ptr::copy_nonoverlapping(left.as_ptr(), data_out, n) };
            f.sent += n;
            if let Some(b) = bytes_read {
                *b = n as i32;
            }
            1
        }

        fn cancel(&self) {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_reference() {
        assert_eq!(super::base64(b"hi"), "aGk=");
        assert_eq!(super::base64(b"<p>x</p>"), "PHA+eDwvcD4=");
        assert_eq!(super::base64(b"abc"), "YWJj");
    }
}
