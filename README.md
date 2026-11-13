# brxswer

A small, flat, telemetry-blocking browser. Chrome is drawn natively with
[Slint](https://slint.dev) (software renderer); pages render in the system
web engine through [wry](https://github.com/tauri-apps/wry) (WebView2 on
Windows, WebKitGTK on Linux). The look follows the
[Orrery](https://github.com/thomasmartinoa/Orrery-dotfiles) Hyprland rice:
0px radius, 1px borders, no blur/shadow/gradient, one monospace size,
80 ms colour snaps, and the four Orrery palettes (Eclipse, Zenith,
Catppuccin Mocha, Cassini) with their exact `colors.toml` values.

## Build & run

```bash
cargo build --release
./target/release/brxswer            # opens an empty tab
./target/release/brxswer github.com news.ycombinator.com
```

Requirements: Rust 1.92+ (Slint 1.18 MSRV). Windows needs the WebView2 runtime (ships with
Windows 10/11). Linux needs `webkit2gtk-4.1` and `gtk3` dev packages.

Set `BRX_TRACE=1` to print every shield event to stderr (debug builds keep
a console).

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| ctrl+l | address bar | alt+left/right | back / forward |
| ctrl+t / ctrl+w | new / close tab | ctrl+r, F5 | reload (stop while loading) |
| ctrl+1..9 | jump to tab | ctrl+b | shield panel |
| ctrl+tab / +shift | next / previous tab | alt+t | next theme |
| esc (in address) | back to page | up/down, tab | pick / complete suggestion |

## Architecture

```
src/
  main.rs            wiring: config → shield → UI → event loop
  app.rs             single owner of state; one message queue
  omnibox.rs         url-or-search resolution, suggestions
  theme.rs           the four Orrery palettes
  config.rs          %APPDATA%/brxswer/config.json (~/.config/brxswer on Linux)
  platform/          engine hooks (WebView2 today; no-op fallback elsewhere)
  shield/
    mod.rs           layers, event bus, ledger, runtime-script builder
    rules.rs         suffix-matched host set + path rules + user filters
    params.rs        tracking-parameter stripping, redirect unwrapping
    lists.rs         built-in rule data
    runtime.js       in-page layer (injected at document start, all frames)
ui/app.slint         the chrome
```

**Event-driven core.** Engine callbacks and UI callbacks never touch state;
they `post` a `Msg`. The queue drains on the event loop and the UI is
refreshed once per batch, with no polling timers. This also makes the app
safe while WebView2 pumps the message loop inside its own calls.

**Lazy engine.** A tab has no web view until it navigates. An empty window
costs ~10 MB private memory and is visible in ~0.25 s; background tabs are
dropped to WebView2's low-memory state.

**The bar is the title bar.** On Windows the native caption is removed
but the frame stays, so Aero Snap, Win+arrows, resize borders and the
minimize animation keep working (`platform/titlebar.rs`). Empty bar space
on either side of the address drags the window (double-click maximizes,
right-click opens the system menu); the maximize cell answers as the
native maximize button, which enables the Windows 11 snap-layout flyout.
Corners are square and the 1px border takes the theme colour. On Linux the
compositor decorates or tiles the window, so the bar shows no controls.

**Tiling, not overlays.** The page is a native child window, so nothing can
be drawn over it. The shield panel and address suggestions *push* the page
instead, like a tiled Hyprland split.

### The shield (defense in depth)

Each layer works on its own and reports through one event bus
(`shield::emit`). The app is the only subscriber and folds events into a
per-tab ledger shown in the panel (`ctrl+b`).

| Layer | Where | What it does |
| --- | --- | --- |
| **T** transport | `platform/windows.rs` | Intercepts every request from every frame and worker (`WebResourceRequested`, all contexts and source kinds). Blocked requests get a *fake, well-formed* response: empty JS for scripts, a 1×1 GIF for pixels, `204` for fetch/XHR/beacon, CORS headers echoed so callers see success and stop retrying. Allowed requests have tracking parameters removed and carry `Sec-GPC: 1`. |
| **N** navigation | `app.rs` | Top-level navigations to tracker hosts are refused with a flat block page. |
| **P** params | `shield/params.rs` | `utm_*`, `fbclid`, `gclid`, `msclkid`, `mc_eid`, HubSpot/Matomo/Yandex/… keys plus site-specific ones (`si` on YouTube, `pd_rd_*` on Amazon) are stripped before the page loads. Click-wrappers like `google.com/url?q=` are unwrapped. Raw query segments are kept byte-for-byte. A loop guard gives up after 3 server re-redirects. |
| **R** runtime | `shield/runtime.js` | `fetch` → fake `204`, XHR → empty `data:` reply, `<img>/<script>/<iframe>` `src` → inert stubs, tracker WebSockets → a socket that closes cleanly. Analytics globals (`ga`, `fbq`, `mixpanel`, `_paq`, `posthog`, …) are no-op proxies until the page assigns its own. `navigator.globalPrivacyControl = true`; `<a ping>` is stripped. |
| **B** beacons | `shield/runtime.js` | `sendBeacon` to tracker hosts is dropped and reports `true`. Beacons elsewhere are **scrambled**: same rough size, random payload (`"beacons": "drop"` in config drops them instead). |

The engine's own phone-home is reduced with Chromium switches
(`--no-pings`, `--disable-domain-reliability`,
`--disable-background-networking`, `--metrics-recording-only`, …; see
`platform::BROWSER_ARGS`). These are best-effort: WebView2 still follows
system diagnostic-data policy.

### Rules

Built-in rules cover Google/Meta/Microsoft telemetry, product analytics and
session replay (Hotjar, FullStory, LogRocket, Mixpanel, Amplitude, Segment,
PostHog, …), error telemetry (Sentry, Bugsnag, …), marketing pixels and the
main ad exchanges and data brokers. Lookups walk the host's labels against a
hash set, so they cost O(labels) per request regardless of list size.

Add your own in `filters.txt` next to `config.json` (created on first run):

```
||tracker.example^          block host + subdomains
0.0.0.0 ads.example         hosts-file syntax works too
cdn.example/collect         host + path prefix
@@||clarity.ms^             allow, overrides built-ins
```

### Config

`config.json`: `theme` (`eclipse` | `zenith` | `catppuccin-mocha` |
`cassini`), `home` (empty = blank tab), `search` (`%s` placeholder, default
DuckDuckGo), `font` (monospace family; `Consolas` on Windows, `JetBrains
Mono` elsewhere), `shield` (master switch), `beacons` (`scramble` | `drop`).

## Stack choice

* **Slint, software renderer.** Native, no GPU context, no JS VM, small
  runtime. The chrome is ~550 lines of declarative UI.
* **wry, the system web engine.** No bundled Chromium, so the ~8.7 MB
  release binary is mostly the UI toolkit and font stack.
* **Dropped** from the old build: egui/eframe, tokio, the `adblock` crate,
  `url`, `regex`, `chrono`, `uuid`, `rand`, the fingerprint profile manager
  and its companion window.

## Platform status

| | Windows (WebView2) | Linux (WebKitGTK) |
| --- | --- | --- |
| Chrome, tabs, themes | ✓ | builds against the same code; not yet tested |
| Custom title bar + window controls | ✓ (snap layouts, square corners) | no: compositor decorations |
| Navigation + params + runtime layers | ✓ | ✓ (engine-independent) |
| Transport layer | ✓ | not yet: wry exposes no subresource hook; planned via WebKit content filters |
| Native shortcuts while page focused | ✓ (`AcceleratorKeyPressed`) | in-page key listener fallback |

## Tests

```bash
cargo test
```

Covers URL splitting, rule matching (built-in + user syntax), parameter
stripping and redirect unwrapping, address resolution and shortcut parsing.
