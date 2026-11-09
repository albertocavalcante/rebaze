//! Validated output intent. Destination state belongs to the application layer.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BundleError {
    #[error("generated output bundle is empty")]
    Empty,
    #[error(
        "output path requires ASCII relative components without traversal, reserved names or ambiguous spelling: {0:?}"
    )]
    InvalidPath(String),
    #[error("output path is declared more than once: {0:?}")]
    Duplicate(String),
    #[error("output paths alias under ASCII case folding: {first:?} and {second:?}")]
    CaseAlias { first: String, second: String },
    #[error("output file {file:?} is also a parent of {descendant:?}")]
    FileAncestor { file: String, descendant: String },
}

/// An ASCII slash-separated relative output path. Construction performs no IO.
/// Components exclude VCS/recovery namespaces, reserved device names and
/// ambiguous trailing dots/spaces.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OutputPath(String);

impl OutputPath {
    pub fn new(path: String) -> Result<Self, BundleError> {
        if path.is_empty() || path.split('/').any(|part| !valid_component(part)) {
            return Err(BundleError::InvalidPath(path));
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

fn valid_component(part: &str) -> bool {
    if part.is_empty()
        || [".git", ".hg", ".svn", ".rebaze-apply"]
            .iter()
            .any(|reserved| part.eq_ignore_ascii_case(reserved))
        || part.ends_with(['.', ' '])
        || part.chars().any(|c| {
            !c.is_ascii()
                || c.is_control()
                || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

impl Borrow<str> for OutputPath {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for OutputPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Immutable ordered files, validated as one set before application is possible.
/// No baseline, host observation, application or verification success is implied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputBundle {
    files: BTreeMap<OutputPath, String>,
}

impl IntoIterator for OutputBundle {
    type Item = (OutputPath, String);
    type IntoIter = std::collections::btree_map::IntoIter<OutputPath, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.files.into_iter()
    }
}

impl OutputBundle {
    pub fn new(files: impl IntoIterator<Item = (String, String)>) -> Result<Self, BundleError> {
        // HashMap callers have no stable iteration order. Validate in path order
        // so failures identify the same offending entry as well as stable success.
        let mut files: Vec<_> = files.into_iter().collect();
        files.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut ordered = BTreeMap::new();
        for (path, content) in files {
            let path = OutputPath::new(path)?;
            if ordered.insert(path.clone(), content).is_some() {
                return Err(BundleError::Duplicate(path.0));
            }
        }
        if ordered.is_empty() {
            return Err(BundleError::Empty);
        }
        let mut folded = BTreeMap::new();
        for path in ordered.keys() {
            let key = path.0.to_ascii_lowercase();
            if let Some(previous) = folded.insert(key, path) {
                return Err(BundleError::CaseAlias {
                    first: previous.0.clone(),
                    second: path.0.clone(),
                });
            }
        }
        for path in ordered.keys() {
            let key = path.0.to_ascii_lowercase();
            for (index, _) in path.0.match_indices('/') {
                let ancestor = &key[..index];
                if let Some(previous) = folded.get(ancestor) {
                    return Err(BundleError::FileAncestor {
                        file: previous.0.clone(),
                        descendant: path.0.clone(),
                    });
                }
            }
        }
        Ok(Self { files: ordered })
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&OutputPath, &str)> {
        self.files
            .iter()
            .map(|(path, content)| (path, content.as_str()))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Compatibility conversion; map iteration does not retain the bundle order.
    pub fn into_files(self) -> HashMap<String, String> {
        self.files
            .into_iter()
            .map(|(path, content)| (path.0, content))
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_or_escaping_paths_before_returning_a_bundle() {
        for path in [
            "",
            "/file",
            "../file",
            "a/../b",
            "a/./b",
            "a//b",
            "a/",
            "C:/file",
            "a\\b",
            "a\0b",
            "NUL",
            "con.txt",
            "sub/AUX.tar.gz",
            "COM1",
            "LPT9.txt",
            "a.",
            "a /b",
            "a?b",
            "a<b",
            "a>b",
            "a\"b",
            "a|b",
            "a*b",
            "café",
        ] {
            assert!(
                matches!(
                    OutputBundle::new([(path.into(), "data".into())]),
                    Err(BundleError::InvalidPath(_))
                ),
                "{path:?}"
            );
        }
        assert_eq!(OutputBundle::new([]), Err(BundleError::Empty));
        assert!(matches!(
            OutputBundle::new([("a".into(), "first".into()), ("a".into(), "second".into())]),
            Err(BundleError::Duplicate(_))
        ));
        // The sibling sorts between the ancestor and descendant, defeating an
        // adjacent-entry-only prefix check.
        assert!(matches!(
            OutputBundle::new([
                ("a".into(), String::new()),
                ("a-sibling".into(), String::new()),
                ("a/b".into(), String::new())
            ]),
            Err(BundleError::FileAncestor { .. })
        ));
        assert!(matches!(
            OutputBundle::new([("A".into(), String::new()), ("a".into(), String::new())]),
            Err(BundleError::CaseAlias { .. })
        ));
        assert!(matches!(
            OutputBundle::new([
                ("A".into(), String::new()),
                ("a/child".into(), String::new())
            ]),
            Err(BundleError::FileAncestor { .. })
        ));
    }

    #[test]
    fn reserved_namespaces_cannot_become_output_intent() {
        for name in [".git", ".hg", ".svn", ".rebaze-apply"] {
            for spelling in [name.to_owned(), name.to_ascii_uppercase()] {
                for path in [
                    spelling.clone(),
                    format!("{spelling}/config"),
                    format!("module/{spelling}/config"),
                ] {
                    assert_eq!(
                        OutputPath::new(path.clone()),
                        Err(BundleError::InvalidPath(path.clone()))
                    );
                    assert_eq!(
                        OutputBundle::new([
                            ("BUILD.bazel".into(), "generated".into()),
                            (path.clone(), "metadata replacement".into()),
                        ]),
                        Err(BundleError::InvalidPath(path))
                    );
                }
            }
        }
    }

    #[test]
    fn input_order_does_not_change_paths_bytes_or_equality() {
        let input = [
            ("z/file".into(), "last\n".into()),
            ("a".into(), "first\n".into()),
        ];
        let forward = OutputBundle::new(input.clone()).unwrap();
        let reverse = OutputBundle::new(input.into_iter().rev()).unwrap();
        assert_eq!(forward, reverse);
        assert_eq!(
            forward
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes))
                .collect::<Vec<_>>(),
            [("a", "first\n"), ("z/file", "last\n")]
        );
        let invalid = [
            ("/absolute".into(), String::new()),
            ("../escape".into(), String::new()),
        ];
        assert_eq!(
            OutputBundle::new(invalid.clone()),
            OutputBundle::new(invalid.into_iter().rev())
        );
    }
}
