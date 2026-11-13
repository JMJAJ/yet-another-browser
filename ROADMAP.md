# brxswer roadmap

The working contract for building brxswer. Claude works through it
milestone by milestone without asking for approval on each step, and stops
only at the decision gates listed below.

Design source of truth: the interactive spec (source in `docs/spec.html`; open it in any browser)
(https://claude.ai/artifact/MdkniVkdpL5vfe2cEmJJYF), 35 states with how each
one works and how to build it. Where code and spec disagree, the spec wins
unless a gate says otherwise.

## North star

A browser that blocks and fakes telemetry at every layer, with a flat
Orrery-style interface that is intuitive on first use, and that stays fast:
the window is visible in under 300 ms and the chrome never stutters.

## Budgets (checked at the end of every milestone)

| Measure | Budget | How it is measured |
| --- | --- | --- |
| Window visible | ≤ 300 ms from launch | `mem.ps1`: Start-Process until MainWindowHandle |
| Empty window, browser process | ≤ 15 MB private | same script, after 6 s idle |
| Engine start | off the critical path | the window never waits for the engine |
| brxswer.exe | ≤ 12 MB (engine files excluded) | release build size |
| Unit tests | all pass | `cargo test` |
| Shield | every layer fires on the tracker test page | `BRX_TRACE=1` run against `brx-test/index.html` |

## Milestones

Order matters: features that talk to the engine (find, downloads,
permissions, context menus, zoom, audio) are built after the engine switch,
so they are written once.

### M1: chrome that does not depend on the engine
- [x] Owned popup windows (borderless, positioned under a cell, close on Esc,
      outside click and focus loss): the base for menus, cards and toasts.
      Non-activating variant for cards and toasts still to add.
- [x] Main menu (≡) with inline theme row; theme cell moved into it.
      Zoom row lands with zoom in M3; more items join as features land.
- [x] Tab states: pinned, audio/muted, trimmed, crashed; hover close glyph;
      middle-click close; hover card after 450 ms.
- [x] Tab context menu; close never confirms; toast with reopen; ctrl+shift+t
      stack of 25. (Reopen restores the page, not its back/forward list:
      neither engine can rebuild navigation history.)
- [x] Drag to reorder; strip scroll with ‹ n / n › when tabs overflow.
      (Tear-off into a new window deferred: needs multi-window support.)
- [x] Address idle display: host bold, rest grey; tag states tls / http /
      cert / brx / blocked; punycode warning. (cert is set by M3's
      certificate handling.)
- [x] Suggestion kinds: go, tab (switch to open tab), hist, find, cmd (>).
- [x] Command palette (ctrl+k) sharing one command table with menus and keys.
- [x] Internal pages as Slint views: history (search, per-day groups),
      settings (5 sections, live apply), empty tab with recently closed.
      Not yet in settings: density, key rebinding, third-party cookies,
      "focus the opener" after close (see "Deferred" below).
- [x] Narrow-window behaviour (< 720 px) and ephemeral window (ctrl+shift+n).
      Ephemeral windows are a separate process with an in-memory profile.
- Done when: every M1 state in the spec matches the app in screenshots,
  budgets hold, committed.

### M2: bundled Chromium (CEF)
- [ ] `Engine` trait in `src/engine/` covering what the app uses today
      (create, navigate, history, focus, bounds, visibility, scripts, IPC,
      title/load events, request interception). WebView2 becomes one
      implementation.
- [ ] CEF implementation via `cef` (tauri-apps/cef-rs): child window in our
      HWND, multi-threaded message loop on Windows, subprocess entry at the
      top of `main`.
- [ ] Transport layer on CEF: `GetResourceRequestHandler` /
      `OnBeforeResourceLoad` for blocking and param stripping, a
      `CefResourceHandler` for fake replies, `Sec-GPC` header.
- [ ] Runtime script injection in every frame, keyboard shortcuts through
      `OnPreKeyEvent`, popup handling through `OnBeforePopup`.
- [ ] Engine telemetry off: no Google API keys, component updater, safe
      browsing, metrics or crash upload (command-line switches + prefs).
- [ ] Packaging: CEF binaries fetched by the build helper, laid out next to
      the exe; release zip.
- Gate: measure install size, idle RAM with one page, and cold start, and
  report them against WebView2 before making CEF the default.
- Done when: the tracker test page passes on CEF, budgets hold (engine files
  excluded), WebView2 removed or kept as fallback per the gate decision.

### M3: features that talk to the engine
- [ ] Find in page bar (tiles at the bottom).
- [ ] Downloads: bar cell with percent, panel (pause, cancel, retry, open,
      show in folder), no engine flyout.
- [ ] Permission strip (camera, mic, location) with per-site memory;
      notifications refused quietly.
- [ ] Site info popover: connection, shield pause, permissions, storage,
      clear site data.
- [ ] Page and link context menus (copy clean link first).
- [ ] Link destination status box with stripped-param preview.
- [ ] Error pages: offline, certificate (no proceed button, `!` override),
      crashed tab; shield block page with "allow this site".
- [ ] Zoom per site, real loading progress, audio state on tabs.
- [ ] Shield: per-site pause (counts as "seen"), filters editor in settings.
- [ ] Fullscreen (F11 + page fullscreen, top-edge reveal).

### M4: Linux / Hyprland
- [ ] Build and run on Linux; compositor decorations, controls hidden under
      tiling compositors.
- [ ] "Follow Orrery": read the current Orrery theme and switch live.
- [ ] Transport layer on Linux through the same CEF backend.

## Deferred

Spec items that need groundwork first; each gets done once its dependency
lands.
- Settings › density (roomy 40 px bar): needs the bar height to become a
  theme variable across the chrome and hit-testing.
- Settings › key rebinding: needs a keymap layer shared with the engine's
  shortcut capture (comes with M2's key handling).
- Settings › third-party cookies: engine setting, lands with M2 (CEF prefs).
- Tab tear-off / "move to new window": needs multi-window support.

## How Claude works through this

Decides alone:
- Implementation details, file layout, names, refactors, tests.
- Visual details the spec leaves open, within its rules (flat, 1px, one
  monospace size, ≤ 100 ms snaps, tones only for state).
- Small dependencies that fit the budgets.
- Order of tasks inside a milestone.

Asks first (decision gates):
- Changing what the spec says a surface does, or adding a surface it lacks.
- Anything that breaks a budget, or a dependency over 2 MB in the exe.
- Changing privacy defaults (what is blocked, kept, or sent).
- Deleting user data or files outside the repo.
- Pushing, opening PRs, publishing, or anything that costs money.
- The M2 engine gate.

Every milestone ends with: build clean, `cargo test`, screenshots of the
affected states, budget check, a commit on the working branch, and a short
report. Progress is ticked off in this file.
