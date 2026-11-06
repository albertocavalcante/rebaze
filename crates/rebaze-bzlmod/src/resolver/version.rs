//! Version comparison utilities for MVS.

/// Compare Bazel module versions, ignoring build metadata.
///
/// Returns -1, 0 or 1. Numeric identifiers sort before nonnumeric ones;
/// shorter equal prefixes sort first. Prereleases sort before releases.
/// See Bazel 9's `bzlmod/Version.java` for the reference ordering.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> i32 {
    match crate::label::compare_version_strings(a, b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_versions() {
        assert!(compare_versions("1.0.0", "0.9.0") > 0);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), 0);
        assert!(compare_versions("1.0.0", "1.0.1") < 0);
        assert!(compare_versions("2.0.0", "1.9.9") > 0);
    }

    #[test]
    fn test_prerelease_versions() {
        assert!(compare_versions("1.0.0-alpha", "1.0.0") < 0);
        assert!(compare_versions("1.0.0", "1.0.0-alpha") > 0);
        assert!(compare_versions("1.0.0-alpha", "1.0.0-beta") < 0);
        assert!(compare_versions("1.0.0-alpha.1", "1.0.0-alpha.2") < 0);
    }

    #[test]
    fn test_different_lengths() {
        assert!(compare_versions("1.0", "1.0.0") < 0);
        assert!(compare_versions("1.0.0.0", "1.0.0") > 0);
        assert!(compare_versions("1.0", "1.0.1") < 0);
    }

    #[test]
    fn test_numeric_comparison() {
        assert!(compare_versions("1.10.0", "1.9.0") > 0);
        assert!(compare_versions("1.2.3", "1.2.10") < 0);
    }

    #[test]
    fn test_bazel_versions() {
        // Common Bazel module versions
        assert!(compare_versions("0.40.0", "0.39.0") > 0);
        assert!(compare_versions("7.0.0", "6.5.0") > 0);
        assert!(compare_versions("1.0.0-rc1", "1.0.0-beta1") > 0);
    }
}
