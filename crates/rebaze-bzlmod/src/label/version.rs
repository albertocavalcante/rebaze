//! Version parsing and comparison.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// A Bazel module version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Version(String);

impl Version {
    /// Create a new version.
    pub fn new(version: impl Into<String>) -> Result<Self, crate::Error> {
        let version = version.into();
        if version.is_empty() {
            return Err(crate::Error::InvalidVersion("empty version".into()));
        }
        let (normalized, build) = version
            .split_once('+')
            .map_or((version.as_str(), None), |(v, b)| (v, Some(b)));
        let (release, prerelease) = split_prerelease(normalized);
        if !valid_identifiers(release, false)
            || prerelease.is_some_and(|part| !valid_identifiers(part, true))
            || build.is_some_and(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
            })
        {
            return Err(crate::Error::InvalidVersion(version));
        }
        Ok(Self(normalized.to_owned()))
    }

    /// Get the version as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for Version {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Version {
    type Error = crate::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Version> for String {
    fn from(value: Version) -> Self {
        value.0
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_version_strings(&self.0, &other.0)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn valid_identifiers(part: &str, hyphens: bool) -> bool {
    part.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || hyphens && b == b'-')
            && (!identifier.bytes().all(|b| b.is_ascii_digit())
                || identifier.parse::<u64>().is_ok())
    })
}

fn split_prerelease(version: &str) -> (&str, Option<&str>) {
    version
        .split_once('-')
        .map_or((version, None), |(release, pre)| (release, Some(pre)))
}

/// Ordering from Bazel 9's Version.java: build metadata is absent from identity.
pub fn compare_version_strings(a: &str, b: &str) -> Ordering {
    let a = a.split('+').next().unwrap_or(a);
    let b = b.split('+').next().unwrap_or(b);
    // Bazel reserves the empty version for non-registry overrides.
    if a.is_empty() || b.is_empty() {
        return a.is_empty().cmp(&b.is_empty());
    }
    let (a_release, a_pre) = split_prerelease(a);
    let (b_release, b_pre) = split_prerelease(b);
    compare_identifiers(a_release, b_release)
        .then_with(|| a_pre.is_none().cmp(&b_pre.is_none()))
        .then_with(|| match (a_pre, b_pre) {
            (Some(a), Some(b)) => compare_identifiers(a, b),
            _ => Ordering::Equal,
        })
}

fn compare_identifiers(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (Some(a), Some(b)) => {
                let ordering = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(a_number), Ok(b_number)) => a_number.cmp(&b_number).then_with(|| a.cmp(b)),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => a.cmp(b),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn order_and_identity_follow_bazel_versions() {
        assert!(Version::new("1.9").unwrap() < Version::new("1.10").unwrap());
        assert!(Version::new("1.0").unwrap() < Version::new("1.0.0").unwrap());
        assert!(Version::new("1.0-9").unwrap() < Version::new("1.0-alpha").unwrap());
        assert_eq!(
            Version::new("1.0+build1").unwrap(),
            Version::new("1.0+build2").unwrap()
        );
    }

    #[test]
    fn deserialize_validates_and_normalizes_versions() {
        let version: Version = serde_json::from_str("\"1.0+build\"").unwrap();
        assert_eq!(version.as_str(), "1.0");
        for value in [
            "",
            "1..0",
            "1.0-",
            "1.0+",
            "1.0/../../etc",
            "18446744073709551616",
        ] {
            assert!(Version::new(value).is_err(), "{value}");
            assert!(
                serde_json::from_str::<Version>(&format!("\"{value}\"")).is_err(),
                "{value}"
            );
        }
    }
}
