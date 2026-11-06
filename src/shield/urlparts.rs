//! Allocation-free URL splitting for the hot path. The web engine hands us
//! already-normalised URLs, so a full WHATWG parser would be wasted work.

#[derive(Debug, Clone, Copy)]
pub struct Parts<'a> {
    pub scheme: &'a str,
    pub host: &'a str,
    /// Path without query or fragment ("" when absent).
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub fragment: Option<&'a str>,
    /// Everything before `?` (or before `#` when there is no query).
    pub base: &'a str,
}

pub fn split(url: &str) -> Option<Parts<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    let after = &rest[auth_end..];
    let (before_frag, fragment) = match after.find('#') {
        Some(i) => (&after[..i], Some(&after[i + 1..])),
        None => (after, None),
    };
    let (path, query) = match before_frag.find('?') {
        Some(i) => (&before_frag[..i], Some(&before_frag[i + 1..])),
        None => (before_frag, None),
    };
    let base_len = scheme.len() + 3 + auth_end + path.len();
    Some(Parts { scheme, host, path, query, fragment, base: &url[..base_len] })
}

/// True when `host` equals `suffix` or ends with `.suffix`.
/// A suffix ending in `.` (e.g. `amazon.`) matches that label anywhere.
pub fn host_matches(host: &str, suffix: &str) -> bool {
    if suffix.ends_with('.') {
        return host.starts_with(suffix) || host.contains(&format!(".{suffix}"));
    }
    host == suffix
        || (host.len() > suffix.len()
            && host.ends_with(suffix)
            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.')
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits() {
        let p = split("https://user@Sub.Example.com:8080/a/b?x=1&y=2#frag").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.host, "Sub.Example.com");
        assert_eq!(p.path, "/a/b");
        assert_eq!(p.query, Some("x=1&y=2"));
        assert_eq!(p.fragment, Some("frag"));
        assert_eq!(p.base, "https://user@Sub.Example.com:8080/a/b");
        let p = split("https://example.com").unwrap();
        assert_eq!((p.host, p.path, p.query), ("example.com", "", None));
        let p = split("http://[::1]:3000/?q").unwrap();
        assert_eq!((p.host, p.query), ("::1", Some("q")));
    }

    #[test]
    fn suffixes() {
        assert!(host_matches("www.google-analytics.com", "google-analytics.com"));
        assert!(host_matches("google-analytics.com", "google-analytics.com"));
        assert!(!host_matches("notgoogle-analytics.com", "google-analytics.com"));
        assert!(host_matches("www.amazon.de", "amazon."));
        assert!(!host_matches("myamazon.de", "amazon."));
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("https%3A%2F%2Fa.b%2F%3Fx%3D1"), "https://a.b/?x=1");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_encode("a b&c"), "a%20b%26c");
    }
}
