#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod engine;
mod omnibox;
mod platform;
mod shield;
mod theme;

slint::include_modules!();

use app::{post, Msg, APP};
use config::Config;
use shield::{Rules, Shield};
use std::rc::Rc;

fn main() -> Result<(), slint::PlatformError> {
    // CEF runs its helper processes (renderer, GPU, network) from this exe
    #[cfg(feature = "engine-cef")]
    if let Some(code) = engine::cef::run_subprocess() {
        std::process::exit(code);
    }
    let ephemeral = std::env::args().any(|a| a == "--ephemeral");
    let mut cfg = Config::load();
    cfg.ephemeral = ephemeral;
    // "clear data on exit: everything" leaves a marker; the engine had its
    // files open at exit, so the folder is erased here, before it starts
    let wipe = config::dir().join("wipe");
    if !ephemeral && wipe.exists() {
        let _ = std::fs::remove_dir_all(config::dir().join("webdata"));
        let _ = std::fs::remove_dir_all(config::dir().join("chromium"));
        let _ = std::fs::remove_file(&wipe);
    }

    let mut rules = Rules::builtin();
    let filters = config::dir().join("filters.txt");
    if !filters.exists() {
        let _ = std::fs::write(&filters, config::FILTERS_TEMPLATE);
    }
    rules.load_file(&filters);
    let shield = Rc::new(Shield::new(rules, cfg.shield));
    shield::subscribe(|ev| post(Msg::Shield(ev)));

    // bundled Chromium starts lazily, on the first navigation (see
    // App::create_webview), so an empty window never waits for it
    #[cfg(feature = "engine-cef")]
    let cef_on = engine::Kind::pick() == engine::Kind::Chromium;
    #[cfg(feature = "engine-cef")]
    {
        engine::cef::set_shield(cfg.shield);
        engine::cef::set_https(cfg.https == "upgrade");
    }

    let ui = AppWindow::new()?;
    ui.on_submit(|s| post(Msg::Submit(s.into())));
    ui.on_edited(|s| post(Msg::Edited(s.into())));
    ui.on_complete(|i| post(Msg::Complete(i.max(0) as usize)));
    ui.on_tab_clicked(|i| post(Msg::TabClicked(i.max(0) as usize)));
    ui.on_new_tab(|| post(Msg::NewTab));
    ui.on_back(|| post(Msg::Back));
    ui.on_forward(|| post(Msg::Forward));
    ui.on_reload(|| post(Msg::Reload));
    ui.on_toggle_panel(|| post(Msg::TogglePanel));
    ui.on_set_shield(|on| post(Msg::SetShield(on)));
    ui.on_page_focus(|| post(Msg::PageFocus));
    ui.on_geometry_changed(|| post(Msg::Geometry));
    ui.on_win_minimize(|| post(Msg::WinMinimize));
    ui.on_win_maximize(|| post(Msg::WinMaximize));
    ui.on_win_close(|| post(Msg::WinClose));
    ui.on_menu_toggle(|| post(Msg::MenuToggle));
    ui.on_tab_close(|i| post(Msg::TabClose(i.max(0) as usize)));
    ui.on_tab_mute(|i| post(Msg::TabMute(i.max(0) as usize)));
    ui.on_tab_menu(|i| post(Msg::TabMenu(i.max(0) as usize)));
    ui.on_tab_hover(|i, on| post(Msg::TabHover(i.max(0) as usize, on)));
    ui.on_tab_drag(|i, x| post(Msg::TabDrag(i.max(0) as usize, x)));
    ui.on_tab_drop(|i, x| post(Msg::TabDrop(i.max(0) as usize, x)));
    ui.on_tab_geom(|i, x, w| post(Msg::TabGeom(i.max(0) as usize, x, w)));
    ui.on_strip_step(|d| post(Msg::StripStep(d)));
    ui.on_strip_wheel(|d| post(Msg::StripWheel(d)));
    ui.on_hist_search(|q| post(Msg::HistSearch(q.into())));
    ui.on_hist_open(|i| post(Msg::HistOpen(i.max(0) as usize)));
    ui.on_hist_forget(|i| post(Msg::HistForget(i.max(0) as usize)));
    ui.on_hist_clear(|| post(Msg::HistClear));
    ui.on_set_section_pick(|i| post(Msg::SetSection(i.max(0) as usize)));
    ui.on_set_value(|k, v| post(Msg::SetValue(k.into(), v.into())));
    ui.on_set_theme_pick(|i| post(Msg::MenuTheme(i.max(0) as usize)));
    ui.on_reopen_recent(|i| post(Msg::ReopenRecent(i.max(0) as usize)));
    ui.on_shortcut(|text, ctrl, alt, shift| match app::combo_from_slint(&text, ctrl, alt, shift) {
        Some(c) => {
            post(Msg::Key(c));
            true
        }
        None => false,
    });

    ui.show()?;
    let urls: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    APP.with(|a| {
        #[allow(unused_mut)]
        let mut app = app::App::new(ui.clone_strong(), cfg, shield);
        #[cfg(feature = "engine-cef")]
        if cef_on {
            app.set_engine(engine::Kind::Chromium);
        }
        *a.borrow_mut() = Some(app)
    });
    post(Msg::Startup(urls));
    slint::run_event_loop()?;

    // Drop webviews before the window goes away.
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.shutdown();
        }
        a.borrow_mut().take()
    });
    #[cfg(feature = "engine-cef")]
    if engine::cef::started() {
        engine::cef::stop();
    }
    if ephemeral {
        // nothing from an ephemeral window survives it
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("brxswer-eph-{}", std::process::id())));
    }
    Ok(())
}
