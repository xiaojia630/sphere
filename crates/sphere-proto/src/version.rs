//! Which Claude Code versions the protocol handling was tested against.

/// Tested from this version up to, not including, `TESTED_BELOW`.
pub const TESTED_MIN: (u64, u64, u64) = (2, 1, 0);
pub const TESTED_BELOW: (u64, u64, u64) = (2, 2, 0);

/// Parse the first `x.y.z` out of `claude --version` output.
pub fn parse(text: &str) -> Option<(u64, u64, u64)> {
    for word in text.split_whitespace() {
        let mut parts = word.trim_start_matches('v').split('.');
        let (Some(a), Some(b), Some(c)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(a), Ok(b), Ok(c)) = (a.parse(), b.parse(), c.parse()) {
            return Some((a, b, c));
        }
    }
    None
}

pub fn in_tested_range(v: (u64, u64, u64)) -> bool {
    v >= TESTED_MIN && v < TESTED_BELOW
}

/// A human sentence about a version outside the range.
pub fn out_of_range_message(v: (u64, u64, u64)) -> String {
    format!(
        "Claude Code {}.{}.{} is outside the tested range ({}.{}.x up to below {}.{}.0); things may not work",
        v.0, v.1, v.2, TESTED_MIN.0, TESTED_MIN.1, TESTED_BELOW.0, TESTED_BELOW.1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse("2.1.301 (Claude Code)"), Some((2, 1, 301)));
        assert_eq!(parse("claude v3.0.1"), Some((3, 0, 1)));
        assert_eq!(parse("none"), None);
        assert!(in_tested_range((2, 1, 301)));
        assert!(!in_tested_range((2, 2, 0)));
        assert!(!in_tested_range((1, 9, 9)));
        assert!(out_of_range_message((1, 5, 0)).contains("1.5.0"));
    }
}
