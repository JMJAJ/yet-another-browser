//! Address-bar input: URL-or-search resolution and suggestions.

use crate::shield::{params, urlparts};

pub fn resolve(input: &str, search: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") {
        return Some(s.to_string());
    }
    let first = s.split(['/', '?', '#']).next().unwrap_or(s);
    let host = first.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map_or(first, |(h, _)| h);
    let local = host == "localhost" || host.ends_with(".localhost") || host.ends_with(".test")
        || host.parse::<std::net::Ipv4Addr>().is_ok();
    if !s.contains(char::is_whitespace) && (local || (host.contains('.') && !host.ends_with('.') && !host.starts_with('.'))) {
        let scheme = if local { "http" } else { "https" };
        return Some(format!("{scheme}://{s}"));
    }
    Some(search.replace("%s", &urlparts::percent_encode(s)))
}

pub struct Row {
    pub tag: &'static str,
    pub text: String,
    pub note: String,
    pub target: String,
}

pub struct Suggestions {
    pub rows: Vec<Row>,
    /// Removed tracking keys of the "go" target, shown as the strip row.
    pub stripped: Vec<String>,
}

pub fn suggest(input: &str, search: &str, history: &[(String, String)], shield_on: bool) -> Suggestions {
    let mut rows = Vec::new();
    let mut stripped = Vec::new();
    let s = input.trim();
    if s.is_empty() {
        return Suggestions { rows, stripped };
    }
    let resolved = resolve(s, search).unwrap_or_default();
    let is_search = resolved.starts_with(&search.replace("%s", ""));
    if !is_search {
        let mut target = resolved.clone();
        if shield_on {
            if let Some(c) = params::clean_navigation(&resolved) {
                target = c.url;
                stripped = c.removed;
            }
        }
        rows.push(Row { tag: "go", text: target.clone(), note: "enter".into(), target });
    }
    let needle = s.to_lowercase();
    for (url, title) in history.iter().rev() {
        if rows.len() >= 5 {
            break;
        }
        if (url.to_lowercase().contains(&needle) || title.to_lowercase().contains(&needle))
            && !rows.iter().any(|r| &r.target == url)
        {
            let text = url.trim_start_matches("https://").trim_start_matches("http://").to_string();
            rows.push(Row { tag: "hist", text: format!("{text}  — {title}"), note: String::new(), target: url.clone() });
        }
    }
    let engine = urlparts::split(search).map_or("search", |p| p.host).trim_start_matches("www.").to_string();
    let target = search.replace("%s", &urlparts::percent_encode(s));
    rows.push(Row { tag: "find", text: format!("search \u{201c}{s}\u{201d}"), note: engine, target });
    Suggestions { rows, stripped }
}

#[cfg(test)]
mod tests {
    use super::*;
    const S: &str = "https://duckduckgo.com/?q=%s";

    #[test]
    fn resolves() {
        assert_eq!(resolve("github.com", S).unwrap(), "https://github.com");
        assert_eq!(resolve("localhost:3000/x", S).unwrap(), "http://localhost:3000/x");
        assert_eq!(resolve("https://a.b/c", S).unwrap(), "https://a.b/c");
        assert_eq!(resolve("rust borrow checker", S).unwrap(), "https://duckduckgo.com/?q=rust%20borrow%20checker");
        assert_eq!(resolve("hello", S).unwrap(), "https://duckduckgo.com/?q=hello");
        assert_eq!(resolve("10.0.0.1", S).unwrap(), "http://10.0.0.1");
    }

    #[test]
    fn suggests_with_strip() {
        let s = suggest("shop.example/i?utm_source=x&id=1", S, &[], true);
        assert_eq!(s.rows[0].target, "https://shop.example/i?id=1");
        assert_eq!(s.stripped, vec!["utm_source"]);
        assert_eq!(s.rows.last().unwrap().tag, "find");
    }
}
