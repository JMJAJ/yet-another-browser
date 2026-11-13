//! WebView2 integration: the transport layer, native shortcut capture,
//! memory trimming and title-bar theming.

use crate::shield::{self, params, urlparts, Layer, Shield, ShieldEvent, Verdict};
use std::rc::Rc;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, AcceleratorKeyPressedEventHandler, FocusChangedEventHandler, WebResourceRequestedEventHandler};
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::Shell::SHCreateMemStream;
use wry::WebViewExtWindows;

/// Chromium switches that silence the engine's own phone-home traffic.
/// wry's defaults (first entry) are repeated because setting args replaces them.
pub const BROWSER_ARGS: &str = concat!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection ",
    "--no-pings ",
    "--disable-domain-reliability ",
    "--disable-background-networking ",
    "--disable-client-side-phishing-detection ",
    "--disable-breakpad ",
    "--disable-crash-reporter ",
    "--metrics-recording-only ",
    "--disable-sync"
);

const GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
    0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02,
    0x01, 0x44, 0x00, 0x3b,
];

fn context_name(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> &'static str {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "frame",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "pixel",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "css",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "fetch",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => "xhr",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "socket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        _ => "other",
    }
}

/// A well-formed, empty reply matching what the page asked for, so tracker
/// loaders fire `onload` and stop retrying instead of erroring.
unsafe fn fake_response(
    env: &ICoreWebView2Environment,
    ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
    origin: Option<String>,
) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
    let cors = match origin {
        Some(o) if !o.is_empty() && o != "null" => {
            format!("Access-Control-Allow-Origin: {o}\nAccess-Control-Allow-Credentials: true\nVary: Origin")
        }
        _ => "Access-Control-Allow-Origin: *".to_string(),
    };
    let (status, reason, ctype, body): (i32, &str, &str, Option<&[u8]>) = match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => (200, "OK", "text/javascript", None),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => (200, "OK", "text/css", None),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => (200, "OK", "text/html", None),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => (200, "OK", "image/gif", Some(GIF)),
        _ => (204, "No Content", "text/plain", None),
    };
    let headers = HSTRING::from(format!("Content-Type: {ctype}\nCache-Control: no-store\n{cors}"));
    let stream = body.and_then(|b| SHCreateMemStream(Some(b)));
    env.CreateWebResourceResponse(stream.as_ref(), status, &HSTRING::from(reason), &headers)
}

/// Transport layer: sees every request from every frame and worker.
pub fn install_transport(wv: &wry::WebView, tab: u64, shield: Rc<Shield>) -> windows::core::Result<()> {
    let core = wv.webview();
    let env = wv.environment();
    unsafe {
        let filter = HSTRING::from("*");
        match core.cast::<ICoreWebView2_22>() {
            Ok(w22) => w22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                &filter,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?,
            Err(_) => core.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?,
        }
        let mut token = 0i64;
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                if !shield.enabled() {
                    return Ok(());
                }
                let req = args.Request()?;
                let mut raw = PWSTR::null();
                req.Uri(&mut raw)?;
                let uri = take_pwstr(raw);
                if !(uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("ws")) {
                    return Ok(());
                }
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
                args.ResourceContext(&mut ctx)?;
                let headers = req.Headers()?;
                match shield.check(&uri) {
                    Verdict::Block(_rule) => {
                        let origin = {
                            let mut v = PWSTR::null();
                            headers.GetHeader(&HSTRING::from("Origin"), &mut v).ok().map(|_| take_pwstr(v))
                        };
                        let resp = fake_response(&env, ctx, origin)?;
                        args.SetResponse(&resp)?;
                        let p = urlparts::split(&uri);
                        shield::emit(ShieldEvent {
                            tab,
                            layer: Layer::Transport,
                            subject: p.map_or_else(String::new, |p| p.host.to_string()),
                            detail: format!("{} · {}", p.map_or("", |p| p.path), context_name(ctx)),
                            count: 1,
                            sticky: false,
                        });
                    }
                    Verdict::Allow => {
                        if ctx != COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                            if let Some(c) = params::strip_generic(&uri) {
                                req.SetUri(&HSTRING::from(c.url.as_str()))?;
                                shield::emit(ShieldEvent {
                                    tab,
                                    layer: Layer::Params,
                                    subject: c.removed.join(", "),
                                    detail: format!(" stripped · {}", context_name(ctx)),
                                    count: c.removed.len() as u32,
                                    sticky: false,
                                });
                            }
                        }
                        headers.SetHeader(&HSTRING::from("Sec-GPC"), &HSTRING::from("1"))?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

pub fn combo(vk: u32, ctrl: bool, alt: bool, shift: bool) -> Option<String> {
    let key = match vk {
        0x30..=0x39 | 0x41..=0x5A => ((vk as u8) as char).to_ascii_lowercase().to_string(),
        0x09 => "Tab".into(),
        0x25 => "ArrowLeft".into(),
        0x27 => "ArrowRight".into(),
        0x74 => "F5".into(),
        _ => return None,
    };
    let s = format!(
        "{}{}{}{}",
        if ctrl { "ctrl+" } else { "" },
        if alt { "alt+" } else { "" },
        if shift { "shift+" } else { "" },
        key
    );
    crate::app::Command::parse(&s).map(|_| s)
}

/// Captures browser shortcuts while the page has keyboard focus. Pages
/// cannot intercept these; they never reach page script.
pub fn install_accelerators(wv: &wry::WebView, on_key: impl Fn(String) + 'static) -> windows::core::Result<()> {
    let controller = wv.controller();
    let mut token = 0i64;
    unsafe {
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                    return Ok(());
                }
                let mut vk = 0u32;
                args.VirtualKey(&mut vk)?;
                let down = |k: i32| GetKeyState(k) < 0;
                if let Some(c) = combo(vk, down(VK_CONTROL.0 as i32), down(VK_MENU.0 as i32), down(VK_SHIFT.0 as i32)) {
                    args.SetHandled(true)?;
                    on_key(c);
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// Tells the app when the page takes keyboard focus, so the chrome can
/// drop its own focus state.
pub fn install_focus(wv: &wry::WebView, on_focus: impl Fn() + 'static) -> windows::core::Result<()> {
    let mut token = 0i64;
    unsafe {
        wv.controller().add_GotFocus(
            &FocusChangedEventHandler::create(Box::new(move |_, _| {
                on_focus();
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

pub fn set_background(wv: &wry::WebView, background: bool) {
    let level = if background { wry::MemoryUsageLevel::Low } else { wry::MemoryUsageLevel::Normal };
    let _ = wv.set_memory_usage_level(level);
}

pub fn set_web_theme(wv: &wry::WebView, dark: bool) {
    let _ = wv.set_theme(if dark { wry::Theme::Dark } else { wry::Theme::Light });
}

/// Audio and crash reports for one tab. `on_audio(playing)`;
/// `on_crash(browser_died)`: false = only this page's process ended.
pub fn install_media(
    wv: &wry::WebView,
    on_audio: impl Fn(bool) + 'static,
    on_crash: impl Fn(bool) + 'static,
) -> windows::core::Result<()> {
    use webview2_com::{IsDocumentPlayingAudioChangedEventHandler, ProcessFailedEventHandler};
    let core = wv.webview();
    unsafe {
        if let Ok(w8) = core.cast::<ICoreWebView2_8>() {
            let mut token = 0i64;
            w8.add_IsDocumentPlayingAudioChanged(
                &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(w) = sender.and_then(|s| s.cast::<ICoreWebView2_8>().ok()) {
                        let mut playing = windows_core::BOOL::default();
                        w.IsDocumentPlayingAudio(&mut playing)?;
                        on_audio(playing.as_bool());
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        let mut token = 0i64;
        core.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                if let Some(a) = args {
                    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                    a.ProcessFailedKind(&mut kind)?;
                    match kind {
                        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => on_crash(true),
                        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                        | COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => on_crash(false),
                        _ => {}
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

pub fn set_muted(wv: &wry::WebView, muted: bool) {
    unsafe {
        if let Ok(w8) = wv.webview().cast::<ICoreWebView2_8>() {
            let _ = w8.SetIsMuted(muted);
        }
    }
}

/// Puts a hidden tab's page to sleep (timers and script paused, memory
/// released). Showing it again resumes it.
pub fn try_suspend(wv: &wry::WebView) {
    use webview2_com::TrySuspendCompletedHandler;
    unsafe {
        if let Ok(w3) = wv.webview().cast::<ICoreWebView2_3>() {
            let _ = w3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(()))));
        }
    }
}

pub fn set_clipboard(text: &str) -> bool {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = (|| -> windows::core::Result<()> {
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            let dst = GlobalLock(mem) as *mut u16;
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0)))?;
            Ok(())
        })()
        .is_ok();
        let _ = CloseClipboard();
        ok
    }
}
