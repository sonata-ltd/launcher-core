//! Java version strings, as `java.runtime.version`, `java.specification.version`
//! and the `JAVA_VERSION` line of a `release` file spell them.

/// The feature (major) release: `1.8.0_402` → 8, `17.0.2+8` → 17, `21` → 21,
/// `22-ea` → 22. `None` for anything that does not start with a number.
pub fn parse_major(version: &str) -> Option<u32> {
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let first: u32 = parts.next()?.parse().ok()?;

    if first == 1 {
        return parts.next()?.parse().ok();
    }

    Some(first)
}

/// Every number in the version, in order: compared lexicographically this
/// ranks builds of one major release — `21.0.5+11` above `21.0.2+13`,
/// `1.8.0_402` above `1.8.0_66`.
pub fn sort_key(version: &str) -> Vec<u32> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majors() {
        assert_eq!(parse_major("1.8.0_402"), Some(8));
        assert_eq!(parse_major("1.8"), Some(8));
        assert_eq!(parse_major("17.0.2+8"), Some(17));
        assert_eq!(parse_major("21"), Some(21));
        assert_eq!(parse_major("22-ea"), Some(22));
        assert_eq!(parse_major("21.0.2+13-LTS"), Some(21));
        assert_eq!(parse_major(""), None);
        assert_eq!(parse_major("openjdk"), None);
    }

    #[test]
    fn builds_of_one_major_order_by_update() {
        assert!(sort_key("21.0.5+11") > sort_key("21.0.2+13"));
        assert!(sort_key("1.8.0_402") > sort_key("1.8.0_66"));
    }
}
