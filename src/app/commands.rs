//! One table of named commands, shared by the address bar's `>` mode, the
//! command palette and the menus, so names and shortcuts never drift apart.

use super::Command;

pub(super) struct Named {
    pub label: &'static str,
    pub key: &'static str,
    pub cmd: Command,
}

pub(super) fn table() -> Vec<Named> {
    let n = |label, key, cmd| Named { label, key, cmd };
    vec![
        n("new tab", "ctrl+t", Command::NewTab),
        n("close tab", "ctrl+w", Command::CloseTab),
        n("reopen closed tab", "ctrl+shift+t", Command::Reopen),
        n("next tab", "ctrl+tab", Command::NextTab),
        n("previous tab", "ctrl+shift+tab", Command::PrevTab),
        n("last tab", "ctrl+9", Command::LastTab),
        n("mute tab", "ctrl+m", Command::Mute),
        n("back", "alt+left", Command::Back),
        n("forward", "alt+right", Command::Forward),
        n("reload", "ctrl+r", Command::Reload),
        n("address bar", "ctrl+l", Command::Address),
        n("command palette", "ctrl+k", Command::Palette),
        n("shield: toggle panel", "ctrl+b", Command::Panel),
        n("theme: next", "alt+t", Command::Theme),
        n("theme: eclipse", "", Command::SetTheme(0)),
        n("theme: zenith", "", Command::SetTheme(1)),
        n("theme: catppuccin mocha", "", Command::SetTheme(2)),
        n("theme: cassini", "", Command::SetTheme(3)),
        n("history", "ctrl+h", Command::History),
        n("settings", "ctrl+,", Command::Settings),
        n("new ephemeral window", "ctrl+shift+n", Command::Ephemeral),
        n("copy clean link", "", Command::CopyLink),
        n("quit", "ctrl+q", Command::Quit),
    ]
}

/// Every word of the query appears in the label, in any order.
pub(super) fn matches(label: &str, query: &str) -> bool {
    let l = label.to_lowercase();
    query.split_whitespace().all(|w| l.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(matches("theme: zenith", "zen th"));
        assert!(matches("reopen closed tab", "tab reo"));
        assert!(!matches("reload", "tab"));
        assert!(matches("anything", ""));
    }

    #[test]
    fn shortcuts_parse() {
        for c in table() {
            if !c.key.is_empty() && !c.key.contains("tab") && c.key != "alt+left" && c.key != "alt+right" {
                assert!(Command::parse(c.key).is_some(), "{} does not parse", c.key);
            }
        }
    }
}
