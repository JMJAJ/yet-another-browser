//! Tracking-parameter stripping and click-redirect unwrapping.
//! Works on the raw query string so untouched parameters keep their exact
//! original encoding.

use super::lists;
use super::urlparts::{host_matches, percent_decode, split};

pub struct Cleaned {
    pub url: String,
    pub removed: Vec<String>,
}

fn is_tracking(host: &str, key: &str, site_rules: bool) -> bool {
    let key = key.to_ascii_lowercase();
    if lists::PARAM_KEYS.contains(&key.as_str()) || lists::PARAM_PREFIXES.iter().any(|p| key.starts_with(p)) {
        return true;
    }
    site_rules
        && lists::PARAM_SITE.iter().any(|(site, keys, prefixes)| {
            host_matches(host, site) && (keys.contains(&key.as_str()) || prefixes.iter().any(|p| key.starts_with(p)))
        })
}

/// Top-level URLs: generic and site-specific keys. Returns the cleaned URL
/// when at least one parameter was removed.
pub fn strip(url: &str) -> Option<Cleaned> {
    strip_with(url, true)
}

/// Subresources: generic keys only. Site-specific short keys (`s`, `t`, `si`)
/// are too likely to be real API arguments below the document level.
pub fn strip_generic(url: &str) -> Option<Cleaned> {
    strip_with(url, false)
}

fn strip_with(url: &str, site_rules: bool) -> Option<Cleaned> {
    let p = split(url)?;
    if !matches!(p.scheme, "http" | "https") {
        return None;
    }
    let query = p.query?;
    let host = p.host.to_ascii_lowercase();
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = Vec::new();
    for pair in query.split('&') {
        let key = pair.split('=').next().unwrap_or("");
        if !key.is_empty() && is_tracking(&host, &percent_decode(key), site_rules) {
            removed.push(percent_decode(key));
        } else if !pair.is_empty() {
            kept.push(pair);
        }
    }
    if removed.is_empty() {
        return None;
    }
    let mut out = String::from(p.base);
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = p.fragment {
        out.push('#');
        out.push_str(f);
    }
    Some(Cleaned { url: out, removed })
}

/// Unwraps known click-tracking redirectors (google.com/url?q=…).
pub fn unwrap_redirect(url: &str) -> Option<String> {
    let p = split(url)?;
    let host = p.host.to_ascii_lowercase();
    let query = p.query?;
    for (h, path, key) in lists::REDIRECTS {
        if p.path == *path && host_matches(&host, h) {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == *key {
                        let target = percent_decode(v);
                        if target.starts_with("http://") || target.starts_with("https://") {
                            return Some(target);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Full top-level clean: unwrap redirect, then strip parameters.
pub fn clean_navigation(url: &str) -> Option<Cleaned> {
    match unwrap_redirect(url) {
        Some(target) => {
            let mut removed = vec!["redirect".to_string()];
            match strip(&target) {
                Some(c) => {
                    removed.extend(c.removed);
                    Some(Cleaned { url: c.url, removed })
                }
                None => Some(Cleaned { url: target, removed }),
            }
        }
        None => strip(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_generic() {
        let c = strip("https://shop.example/item/88?utm_source=nl&utm_campaign=fall&fbclid=Iw&color=teal#top").unwrap();
        assert_eq!(c.url, "https://shop.example/item/88?color=teal#top");
        assert_eq!(c.removed, vec!["utm_source", "utm_campaign", "fbclid"]);
        let c = strip("https://a.example/?gclid=1").unwrap();
        assert_eq!(c.url, "https://a.example/");
        assert!(strip("https://a.example/?q=hello%20world&page=2").is_none());
        assert!(strip("https://a.example/").is_none());
    }

    #[test]
    fn strips_site_specific() {
        let c = strip("https://youtu.be/abc?si=XYZ&t=10").unwrap();
        assert_eq!(c.url, "https://youtu.be/abc?t=10");
        assert!(strip("https://example.com/?si=1").is_none());
    }

    #[test]
    fn unwraps() {
        let c = clean_navigation("https://www.google.com/url?sa=t&url=https%3A%2F%2Fexample.com%2Fp%3Futm_source%3Dg%26id%3D7").unwrap();
        assert_eq!(c.url, "https://example.com/p?id=7");
    }
}
