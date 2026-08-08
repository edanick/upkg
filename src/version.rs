//! OS version comparison (Section 7.2 of the spec).
//!
//! The spec says bounds are inclusive: `host >= min && host <= max`.
//! Version strings such as `22.04`, `10.0.19045`, `14.5` are compared with a
//! natural ordering: numeric segments compare numerically, other segments
//! compare lexically. Unknown/empty versions compare as equal to anything
//! (a missing host version cannot be proven to mismatch).

/// A parsed version: a list of segments, each either numeric or textual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    segments: Vec<Seg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Num(u64),
    Text(String),
}

impl Version {
    /// Parse a version string into segments.
    pub fn parse(input: &str) -> Version {
        let mut segments = Vec::new();
        for chunk in input.split(|c: char| !c.is_alphanumeric() && c != '.') {
            if chunk.is_empty() {
                continue;
            }
            if chunk.chars().all(|c| c.is_ascii_digit()) {
                segments.push(Seg::Num(chunk.parse().unwrap_or(0)));
            } else {
                segments.push(Seg::Text(chunk.to_string()));
            }
        }
        Version { segments }
    }

    /// True when the version is empty (not detectable / not declared).
    pub fn is_unknown(&self) -> bool {
        self.segments.is_empty()
    }
}

fn cmp_seg(a: &Seg, b: &Seg) -> std::cmp::Ordering {
    match (a, b) {
        (Seg::Num(x), Seg::Num(y)) => x.cmp(y),
        (Seg::Text(x), Seg::Text(y)) => x.cmp(y),
        // A number sorts before text.
        (Seg::Num(_), Seg::Text(_)) => std::cmp::Ordering::Less,
        (Seg::Text(_), Seg::Num(_)) => std::cmp::Ordering::Greater,
    }
}

/// Compare two versions with natural ordering.
pub fn compare(a: &Version, b: &Version) -> std::cmp::Ordering {
    let mut i = 0;
    loop {
        let sa = a.segments.get(i);
        let sb = b.segments.get(i);
        match (sa, sb) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = cmp_seg(x, y);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                i += 1;
            }
        }
    }
}

/// Check whether `host` satisfies the (inclusive) bounds `min`/`max`.
///
/// Bounds semantics (revisions 4, 23, 24):
/// - no bounds: any version;
/// - only `min`: `host >= min`;
/// - only `max`: `host <= max`;
/// - both: `host >= min && host <= max`.
/// An unknown host version always satisfies the bounds (cannot be disproven).
pub fn in_range(host: &str, min: Option<&str>, max: Option<&str>) -> bool {
    let host = Version::parse(host);
    if host.is_unknown() {
        return true;
    }
    if let Some(m) = min {
        if compare(&host, &Version::parse(m)) == std::cmp::Ordering::Less {
            return false;
        }
    }
    if let Some(m) = max {
        if compare(&host, &Version::parse(m)) == std::cmp::Ordering::Greater {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        assert!(compare(&Version::parse("2"), &Version::parse("10")) == std::cmp::Ordering::Less);
        assert!(compare(&Version::parse("22.04"), &Version::parse("24.04")) == std::cmp::Ordering::Less);
        assert!(compare(&Version::parse("10.0.19045"), &Version::parse("10.0.19041")) == std::cmp::Ordering::Greater);
        assert!(compare(&Version::parse("14.5"), &Version::parse("14.5")) == std::cmp::Ordering::Equal);
        assert!(compare(&Version::parse("14.5.1"), &Version::parse("14.5")) == std::cmp::Ordering::Greater);
    }

    #[test]
    fn bounds() {
        // min only: "since"
        assert!(in_range("24.04", Some("22.04"), None));
        assert!(!in_range("20.04", Some("22.04"), None));
        // max only: "until"
        assert!(in_range("20.04", None, Some("22.04")));
        assert!(!in_range("24.04", None, Some("22.04")));
        // both, inclusive
        assert!(in_range("22.04", Some("22.04"), Some("24.04")));
        assert!(in_range("24.04", Some("22.04"), Some("24.04")));
        assert!(!in_range("25.04", Some("22.04"), Some("24.04")));
        // none
        assert!(in_range("anything", None, None));
        // unknown host always passes
        assert!(in_range("", Some("22.04"), Some("24.04")));
    }
}
