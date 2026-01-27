//! Version comparison utilities for MVS.
//!
//! Implements Bazel module version comparison semantics, which follow a
//! simplified semver-like ordering.

use std::cmp::Ordering;

/// Compare two version strings.
///
/// Returns:
/// - Positive if `a > b`
/// - Zero if `a == b`
/// - Negative if `a < b`
///
/// Version comparison follows these rules:
/// 1. Split by dots
/// 2. Compare each segment numerically if both are numbers
/// 3. Compare lexicographically otherwise
/// 4. Pre-release versions (containing `-`) sort before their release counterparts
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> i32 {
    // Handle pre-release versions
    let (a_base, a_pre) = split_prerelease(a);
    let (b_base, b_pre) = split_prerelease(b);

    // Compare base versions first
    let base_cmp = compare_base_versions(a_base, b_base);
    if base_cmp != 0 {
        return base_cmp;
    }

    // If base versions are equal, compare pre-release
    // No pre-release is greater than any pre-release
    match (a_pre, b_pre) {
        (None, None) => 0,
        (None, Some(_)) => 1,  // a is release, b is pre-release
        (Some(_), None) => -1, // a is pre-release, b is release
        (Some(a_pre), Some(b_pre)) => compare_prerelease(a_pre, b_pre),
    }
}

/// Split version into base and pre-release parts.
fn split_prerelease(version: &str) -> (&str, Option<&str>) {
    if let Some(idx) = version.find('-') {
        (&version[..idx], Some(&version[idx + 1..]))
    } else {
        (version, None)
    }
}

/// Compare base version strings (without pre-release).
fn compare_base_versions(a: &str, b: &str) -> i32 {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();

    let max_len = a_parts.len().max(b_parts.len());

    for i in 0..max_len {
        let a_part = a_parts.get(i).copied().unwrap_or("0");
        let b_part = b_parts.get(i).copied().unwrap_or("0");

        let cmp = compare_parts(a_part, b_part);
        if cmp != 0 {
            return cmp;
        }
    }

    0
}

/// Compare two version parts.
fn compare_parts(a: &str, b: &str) -> i32 {
    // Try to parse as numbers first
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(a_num), Ok(b_num)) => match a_num.cmp(&b_num) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        },
        // If either is not a number, compare lexicographically
        _ => match a.cmp(b) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        },
    }
}

/// Compare pre-release identifiers.
fn compare_prerelease(a: &str, b: &str) -> i32 {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();

    let max_len = a_parts.len().max(b_parts.len());

    for i in 0..max_len {
        let a_part = a_parts.get(i).copied();
        let b_part = b_parts.get(i).copied();

        match (a_part, b_part) {
            (None, None) => return 0,
            (None, Some(_)) => return -1, // Fewer pre-release parts = lower
            (Some(_), None) => return 1,
            (Some(a), Some(b)) => {
                let cmp = compare_parts(a, b);
                if cmp != 0 {
                    return cmp;
                }
            }
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_versions() {
        assert!(compare_versions("1.0.0", "0.9.0") > 0);
        assert!(compare_versions("1.0.0", "1.0.0") == 0);
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
        assert!(compare_versions("1.0", "1.0.0") == 0);
        assert!(compare_versions("1.0.0.0", "1.0.0") == 0);
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
