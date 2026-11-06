//! Block rules: a suffix-matched host set plus a few path rules.
//! Lookups walk the host's label suffixes, so cost is O(labels), not O(rules).

use super::lists;
use super::urlparts::{host_matches, split};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Blocked; carries the rule that matched (a host or path pattern).
    Block(String),
}

pub struct Rules {
    domains: HashSet<String>,
    allow: HashSet<String>,
    host_paths: Vec<(String, String)>,
}

impl Rules {
    pub fn builtin() -> Self {
        Rules {
            domains: lists::DOMAINS.iter().map(|s| s.to_string()).collect(),
            allow: HashSet::new(),
            host_paths: lists::HOST_PATHS.iter().map(|(h, p)| (h.to_string(), p.to_string())).collect(),
        }
    }

    /// Load user rules. Accepted line forms:
    /// `example.com`, `||example.com^`, `0.0.0.0 example.com` (hosts file),
    /// `@@||example.com^` (allow), `example.com/path` (host + path prefix).
    /// `!` and `#` start comments.
    pub fn load_file(&mut self, path: &Path) -> usize {
        let Ok(text) = std::fs::read_to_string(path) else { return 0 };
        self.load_str(&text)
    }

    pub fn load_str(&mut self, text: &str) -> usize {
        let mut n = 0;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('!') || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let (allow, line) = match line.strip_prefix("@@") {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            // hosts-file form
            let line = match line.split_once(char::is_whitespace) {
                Some((ip, host)) if ip.parse::<std::net::IpAddr>().is_ok() => host.trim(),
                _ => line,
            };
            let line = line.trim_start_matches("||").trim_start_matches('|');
            let line = line.split('$').next().unwrap_or(line).trim_end_matches('^');
            let (host, path) = match line.find('/') {
                Some(i) => (&line[..i], &line[i..]),
                None => (line, ""),
            };
            let host = host.trim_start_matches("*.").to_ascii_lowercase();
            if host.is_empty() || !host.contains('.') || host == "localhost"
                || !host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
            {
                continue;
            }
            if allow {
                self.allow.insert(host);
            } else if path.is_empty() || path == "/" {
                self.domains.insert(host);
            } else {
                self.host_paths.push((host, path.trim_end_matches('*').to_string()));
            }
            n += 1;
        }
        n
    }

    pub fn domain_list(&self) -> Vec<&str> {
        self.domains.iter().map(String::as_str).collect()
    }

    pub fn host_path_list(&self) -> &[(String, String)] {
        &self.host_paths
    }

    fn suffix_hit<'s>(set: &'s HashSet<String>, host: &str) -> Option<&'s String> {
        let mut h = host;
        loop {
            if let Some(hit) = set.get(h) {
                return Some(hit);
            }
            match h.find('.') {
                Some(i) => h = &h[i + 1..],
                None => return None,
            }
        }
    }

    pub fn check(&self, url: &str) -> Verdict {
        let Some(p) = split(url) else { return Verdict::Allow };
        if !matches!(p.scheme, "http" | "https" | "ws" | "wss") {
            return Verdict::Allow;
        }
        let lowered;
        let host = if p.host.bytes().any(|c| c.is_ascii_uppercase()) {
            lowered = p.host.to_ascii_lowercase();
            lowered.as_str()
        } else {
            p.host
        };
        if Self::suffix_hit(&self.allow, host).is_some() {
            return Verdict::Allow;
        }
        if let Some(hit) = Self::suffix_hit(&self.domains, host) {
            return Verdict::Block(hit.clone());
        }
        for (h, path) in &self.host_paths {
            if p.path.starts_with(path.as_str()) && host_matches(host, h) {
                return Verdict::Block(format!("{h}{path}"));
            }
        }
        for pat in lists::PATH_CONTAINS {
            if p.path.contains(pat) {
                return Verdict::Block(format!("*{pat}"));
            }
        }
        for pat in lists::PATH_SUFFIX {
            if p.path.ends_with(pat) {
                return Verdict::Block(format!("*{pat}"));
            }
        }
        Verdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_blocks() {
        let r = Rules::builtin();
        assert!(matches!(r.check("https://www.google-analytics.com/g/collect?v=2"), Verdict::Block(_)));
        assert!(matches!(r.check("https://region1.analytics.google.com/x"), Verdict::Block(_)));
        assert!(matches!(r.check("https://www.facebook.com/tr?id=1"), Verdict::Block(_)));
        assert!(matches!(r.check("https://cdn.example.com/js/gtag/js"), Verdict::Block(_)));
        assert_eq!(r.check("https://www.facebook.com/profile"), Verdict::Allow);
        assert_eq!(r.check("https://github.com/"), Verdict::Allow);
        assert_eq!(r.check("data:text/plain,hi"), Verdict::Allow);
    }

    #[test]
    fn user_rules() {
        let mut r = Rules::builtin();
        let n = r.load_str("! comment\n||tracker.test^\n0.0.0.0 ads.test\n@@||clarity.ms^\nsite.test/beacon\n");
        assert_eq!(n, 4);
        assert!(matches!(r.check("https://x.tracker.test/"), Verdict::Block(_)));
        assert!(matches!(r.check("https://ads.test/a"), Verdict::Block(_)));
        assert_eq!(r.check("https://www.clarity.ms/tag"), Verdict::Allow);
        assert!(matches!(r.check("https://site.test/beacon/1"), Verdict::Block(_)));
        assert_eq!(r.check("https://site.test/home"), Verdict::Allow);
    }
}
