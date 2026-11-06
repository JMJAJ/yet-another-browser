#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
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
    let cfg = Config::load();

    let mut rules = Rules::builtin();
    let filters = config::dir().join("filters.txt");
    if !filters.exists() {
        let _ = std::fs::write(&filters, config::FILTERS_TEMPLATE);
    }
    rules.load_file(&filters);
    let shield = Rc::new(Shield::new(rules, cfg.shield));
    let trace = std::env::var_os("BRX_TRACE").is_some();
    shield::subscribe(move |ev| {
        if trace {
            eprintln!("[shield] tab{} {} {}{}", ev.tab, ev.layer.letter(), ev.subject, ev.detail);
        }
        post(Msg::Shield(ev))
    });

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
    ui.on_cycle_theme(|| post(Msg::CycleTheme));
    ui.on_set_shield(|on| post(Msg::SetShield(on)));
    ui.on_page_focus(|| post(Msg::PageFocus));
    ui.on_geometry_changed(|| post(Msg::Geometry));
    ui.on_shortcut(|text, ctrl, alt, shift| match app::combo_from_slint(&text, ctrl, alt, shift) {
        Some(c) => {
            post(Msg::Key(c));
            true
        }
        None => false,
    });

    ui.show()?;
    let urls: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    APP.with(|a| *a.borrow_mut() = Some(app::App::new(ui.clone_strong(), cfg, shield)));
    post(Msg::Startup(urls));
    slint::run_event_loop()?;

    // Drop webviews before the window goes away.
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}
