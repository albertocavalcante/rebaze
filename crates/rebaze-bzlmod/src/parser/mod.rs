//! MODULE.bazel file parsing.
//!
//! This module provides a regex-based parser for MODULE.bazel files.
//! It extracts module declarations, dependencies, and overrides without
//! requiring a full Starlark parser.

use crate::{Dependency, ModuleInfo, Override, Result};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// A parsed MODULE.bazel file.
#[derive(Debug, Clone)]
pub struct ModuleFile {
    /// Parsed module information.
    pub info: ModuleInfo,
    /// Original content.
    pub content: String,
}

/// Parser errors with position information.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// Line number (1-indexed).
    pub line: usize,
    /// Column number (1-indexed).
    pub column: usize,
    /// Error message.
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

// Regex patterns for parsing MODULE.bazel
// These patterns are compile-time constant strings, so unwrap is safe
#[allow(clippy::unwrap_used)]
static COMMENT_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#[^\n]*").unwrap());

// Pattern to match function calls like module(...), bazel_dep(...), etc.
// This handles multi-line calls by matching balanced parentheses
#[allow(clippy::unwrap_used)]
static FUNC_CALL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)(module|bazel_dep|single_version_override|git_override|local_path_override|archive_override|multiple_version_override)\s*\(")
        .unwrap()
});

// Pattern for extracting named string arguments: name = "value" or name = 'value'
#[allow(clippy::unwrap_used)]
static STRING_ARG_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\w+)\s*=\s*(?:"([^"\\]*(?:\\.[^"\\]*)*)"|'([^'\\]*(?:\\.[^'\\]*)*)')"#).unwrap()
});

// Pattern for extracting named integer arguments: name = 123
#[allow(clippy::unwrap_used)]
static INT_ARG_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w+)\s*=\s*(-?\d+)").unwrap());

// Pattern for extracting named boolean arguments: name = True/False
#[allow(clippy::unwrap_used)]
static BOOL_ARG_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w+)\s*=\s*(True|False)").unwrap());

// Pattern for extracting string list arguments: name = ["a", "b", "c"]
#[allow(clippy::unwrap_used)]
static STRING_LIST_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w+)\s*=\s*\[([^\]]*)\]").unwrap());

// Pattern for extracting individual strings from a list
#[allow(clippy::unwrap_used)]
static STRING_IN_LIST_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:"([^"\\]*(?:\\.[^"\\]*)*)"|'([^'\\]*(?:\\.[^'\\]*)*)')"#).unwrap()
});

impl ModuleFile {
    /// Parse `MODULE.bazel` content.
    ///
    /// # Errors
    ///
    /// Returns an error if the content is invalid or missing required declarations.
    pub fn parse(content: &str) -> Result<Self> {
        let parser = Parser::new(content);
        parser.parse()
    }

    /// Parse `MODULE.bazel` from file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or the content is invalid.
    pub async fn parse_file(path: &std::path::Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(crate::Error::Io)?;
        Self::parse(&content)
    }
}

/// Internal parser state.
struct Parser<'a> {
    content: &'a str,
    /// Content with comments stripped for easier parsing.
    stripped: String,
}

// Parser implementation uses unwrap on regex captures that are guaranteed to exist by pattern design
#[allow(clippy::unwrap_used)]
impl<'a> Parser<'a> {
    fn new(content: &'a str) -> Self {
        // Strip comments for easier parsing
        let stripped = COMMENT_PATTERN.replace_all(content, "").to_string();
        Self { content, stripped }
    }

    fn parse(self) -> Result<ModuleFile> {
        let mut info = ModuleInfo {
            name: crate::label::ModuleName::new("_root_")?,
            version: crate::label::Version::new("0.0.0")?,
            compatibility_level: 0,
            bazel_compatibility: Vec::new(),
            deps: Vec::new(),
            dev_deps: Vec::new(),
            overrides: Vec::new(),
        };

        let mut found_module = false;

        // Find all function calls
        for cap in FUNC_CALL_PATTERN.captures_iter(&self.stripped) {
            let func_name = cap.get(1).unwrap().as_str();
            let start_pos = cap.get(0).unwrap().end();

            // Find the matching closing parenthesis
            if let Some(args_str) = self.extract_balanced_parens(start_pos) {
                match func_name {
                    "module" => {
                        found_module = true;
                        self.parse_module(&args_str, &mut info)?;
                    }
                    "bazel_dep" => {
                        if let Some(dep) = self.parse_bazel_dep(&args_str)? {
                            if dep.dev_dependency {
                                info.dev_deps.push(dep);
                            } else {
                                info.deps.push(dep);
                            }
                        }
                    }
                    "single_version_override" => {
                        if let Some(ov) = self.parse_single_version_override(&args_str)? {
                            info.overrides.push(ov);
                        }
                    }
                    "git_override" => {
                        if let Some(ov) = self.parse_git_override(&args_str)? {
                            info.overrides.push(ov);
                        }
                    }
                    "local_path_override" => {
                        if let Some(ov) = self.parse_local_path_override(&args_str)? {
                            info.overrides.push(ov);
                        }
                    }
                    "archive_override" => {
                        if let Some(ov) = self.parse_archive_override(&args_str)? {
                            info.overrides.push(ov);
                        }
                    }
                    "multiple_version_override" => {
                        if let Some(ov) = self.parse_multiple_version_override(&args_str)? {
                            info.overrides.push(ov);
                        }
                    }
                    _ => {}
                }
            }
        }

        if !found_module {
            return Err(crate::Error::Parse(
                "no module() declaration found".to_string(),
            ));
        }

        Ok(ModuleFile {
            info,
            content: self.content.to_string(),
        })
    }

    /// Extract content between balanced parentheses starting at the given position.
    fn extract_balanced_parens(&self, start: usize) -> Option<String> {
        let bytes = self.stripped.as_bytes();
        let mut depth = 1;
        let mut pos = start;
        let mut in_string = false;
        let mut string_char = b'"';
        let mut prev_char = b' ';

        while pos < bytes.len() && depth > 0 {
            let ch = bytes[pos];

            if in_string {
                if ch == string_char && prev_char != b'\\' {
                    in_string = false;
                }
            } else {
                match ch {
                    b'"' | b'\'' => {
                        in_string = true;
                        string_char = ch;
                    }
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
            }

            prev_char = ch;
            pos += 1;
        }

        if depth == 0 {
            // Return content inside parens (excluding the closing paren)
            Some(self.stripped[start..pos - 1].to_string())
        } else {
            None
        }
    }

    /// Extract string arguments from a function call body.
    // Uses &self for API consistency with other Parser methods
    #[allow(clippy::unused_self, clippy::unwrap_used)]
    fn extract_string_args(&self, args_str: &str) -> HashMap<String, String> {
        let mut result = HashMap::new();
        for cap in STRING_ARG_PATTERN.captures_iter(args_str) {
            let name = cap.get(1).unwrap().as_str();
            // Either double-quoted or single-quoted string
            let value = cap
                .get(2)
                .or_else(|| cap.get(3))
                .map(|m| unescape_string(m.as_str()))
                .unwrap_or_default();
            result.insert(name.to_string(), value);
        }
        result
    }

    /// Extract integer arguments from a function call body.
    // Uses &self for API consistency with other Parser methods
    #[allow(clippy::unused_self, clippy::unwrap_used)]
    fn extract_int_args(&self, args_str: &str) -> HashMap<String, i64> {
        let mut result = HashMap::new();
        for cap in INT_ARG_PATTERN.captures_iter(args_str) {
            let name = cap.get(1).unwrap().as_str();
            if let Ok(value) = cap.get(2).unwrap().as_str().parse() {
                result.insert(name.to_string(), value);
            }
        }
        result
    }

    /// Extract boolean arguments from a function call body.
    // Uses &self for API consistency with other Parser methods
    #[allow(clippy::unused_self, clippy::unwrap_used)]
    fn extract_bool_args(&self, args_str: &str) -> HashMap<String, bool> {
        let mut result = HashMap::new();
        for cap in BOOL_ARG_PATTERN.captures_iter(args_str) {
            let name = cap.get(1).unwrap().as_str();
            let value = cap.get(2).unwrap().as_str() == "True";
            result.insert(name.to_string(), value);
        }
        result
    }

    /// Extract string list arguments from a function call body.
    // Uses &self for API consistency with other Parser methods
    #[allow(clippy::unused_self, clippy::unwrap_used)]
    fn extract_string_list_args(&self, args_str: &str) -> HashMap<String, Vec<String>> {
        let mut result = HashMap::new();
        for cap in STRING_LIST_PATTERN.captures_iter(args_str) {
            let name = cap.get(1).unwrap().as_str();
            let list_content = cap.get(2).unwrap().as_str();

            let mut values = Vec::new();
            for string_cap in STRING_IN_LIST_PATTERN.captures_iter(list_content) {
                let value = string_cap
                    .get(1)
                    .or_else(|| string_cap.get(2))
                    .map(|m| unescape_string(m.as_str()))
                    .unwrap_or_default();
                values.push(value);
            }
            result.insert(name.to_string(), values);
        }
        result
    }

    /// Parse `module()` declaration.
    fn parse_module(&self, args_str: &str, info: &mut ModuleInfo) -> Result<()> {
        let strings = self.extract_string_args(args_str);
        let ints = self.extract_int_args(args_str);
        let lists = self.extract_string_list_args(args_str);

        if let Some(name) = strings.get("name") {
            info.name = crate::label::ModuleName::new(name.clone())?;
        }

        if let Some(version) = strings.get("version") {
            info.version = crate::label::Version::new(version.clone())?;
        }

        if let Some(&level) = ints.get("compatibility_level") {
            info.compatibility_level = u32::try_from(level).unwrap_or(0);
        }

        if let Some(compat) = lists.get("bazel_compatibility") {
            info.bazel_compatibility.clone_from(compat);
        }

        Ok(())
    }

    /// Parse `bazel_dep()` declaration.
    fn parse_bazel_dep(&self, args_str: &str) -> Result<Option<Dependency>> {
        let strings = self.extract_string_args(args_str);
        let bools = self.extract_bool_args(args_str);
        let ints = self.extract_int_args(args_str);

        let name = match strings.get("name") {
            Some(n) => crate::label::ModuleName::new(n.clone())?,
            None => {
                return Err(crate::Error::Parse(
                    "bazel_dep requires 'name' attribute".to_string(),
                ));
            }
        };

        // Version is optional when using overrides
        let version = match strings.get("version") {
            Some(v) => crate::label::Version::new(v.clone())?,
            None => crate::label::Version::new("0.0.0")?,
        };

        let max_version = strings
            .get("max_version")
            .map(|v| crate::label::Version::new(v.clone()))
            .transpose()?;

        let repo_name = strings.get("repo_name").cloned();
        let dev_dependency = bools.get("dev_dependency").copied().unwrap_or(false);

        // Note: max_compatibility_level is parsed but not used in Dependency struct
        // It's available in ints if needed
        let _ = ints.get("max_compatibility_level");

        Ok(Some(Dependency {
            name,
            version,
            max_version,
            repo_name,
            dev_dependency,
        }))
    }

    /// Parse `single_version_override()` declaration.
    fn parse_single_version_override(&self, args_str: &str) -> Result<Option<Override>> {
        let strings = self.extract_string_args(args_str);

        let module_name = match strings.get("module_name") {
            Some(n) => crate::label::ModuleName::new(n.clone())?,
            None => return Ok(None),
        };

        let version = match strings.get("version") {
            Some(v) => crate::label::Version::new(v.clone())?,
            None => crate::label::Version::new("0.0.0")?,
        };

        let registry = strings.get("registry").cloned();

        Ok(Some(Override::SingleVersion {
            module: module_name,
            version,
            registry,
        }))
    }

    /// Parse `git_override()` declaration.
    fn parse_git_override(&self, args_str: &str) -> Result<Option<Override>> {
        let strings = self.extract_string_args(args_str);

        let module_name = match strings.get("module_name") {
            Some(n) => crate::label::ModuleName::new(n.clone())?,
            None => return Ok(None),
        };

        let remote = strings.get("remote").cloned().unwrap_or_default();
        let commit = strings.get("commit").cloned();
        let tag = strings.get("tag").cloned();
        let branch = strings.get("branch").cloned();

        Ok(Some(Override::Git {
            module: module_name,
            remote,
            commit,
            tag,
            branch,
        }))
    }

    /// Parse `local_path_override()` declaration.
    fn parse_local_path_override(&self, args_str: &str) -> Result<Option<Override>> {
        let strings = self.extract_string_args(args_str);

        let module_name = match strings.get("module_name") {
            Some(n) => crate::label::ModuleName::new(n.clone())?,
            None => return Ok(None),
        };

        let path = strings.get("path").cloned().unwrap_or_default();

        Ok(Some(Override::LocalPath {
            module: module_name,
            path,
        }))
    }

    /// Parse `archive_override()` declaration.
    fn parse_archive_override(&self, args_str: &str) -> Result<Option<Override>> {
        let strings = self.extract_string_args(args_str);
        let lists = self.extract_string_list_args(args_str);

        let module_name = match strings.get("module_name") {
            Some(n) => crate::label::ModuleName::new(n.clone())?,
            None => return Ok(None),
        };

        let urls = lists.get("urls").cloned().unwrap_or_default();
        let integrity = strings.get("integrity").cloned();
        let strip_prefix = strings.get("strip_prefix").cloned();

        Ok(Some(Override::Archive {
            module: module_name,
            urls,
            integrity,
            strip_prefix,
        }))
    }

    /// Parse `multiple_version_override()` declaration.
    fn parse_multiple_version_override(&self, args_str: &str) -> Result<Option<Override>> {
        let strings = self.extract_string_args(args_str);
        let lists = self.extract_string_list_args(args_str);

        let module_name = match strings.get("module_name") {
            Some(n) => crate::label::ModuleName::new(n.clone())?,
            None => return Ok(None),
        };

        let version_strings = lists.get("versions").cloned().unwrap_or_default();
        let mut versions = Vec::new();
        for v in version_strings {
            versions.push(crate::label::Version::new(v)?);
        }

        let registry = strings.get("registry").cloned();

        Ok(Some(Override::MultipleVersion {
            module: module_name,
            versions,
            registry,
        }))
    }
}

/// Unescape a string literal (handle `\n`, `\t`, `\\`, `\"`, `\'`)
fn unescape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') | None => result.push('\\'),
                Some('"') => result.push('"'),
                Some('\'') => result.push('\''),
                Some('0') => result.push('\0'),
                Some(c) => {
                    // Unknown escape, keep as-is
                    result.push('\\');
                    result.push(c);
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_module() {
        let content = r#"
module(
    name = "my_project",
    version = "1.0.0",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.name.as_str(), "my_project");
        assert_eq!(result.info.version.as_str(), "1.0.0");
    }

    #[test]
    fn test_parse_module_with_compatibility() {
        let content = r#"
module(
    name = "my_project",
    version = "2.0.0",
    compatibility_level = 2,
    bazel_compatibility = [">=7.0.0"],
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.name.as_str(), "my_project");
        assert_eq!(result.info.version.as_str(), "2.0.0");
        assert_eq!(result.info.compatibility_level, 2);
        assert_eq!(result.info.bazel_compatibility, vec![">=7.0.0"]);
    }

    #[test]
    fn test_parse_bazel_dep() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_rust", version = "0.40.0")
bazel_dep(name = "rules_go", version = "0.41.0", repo_name = "io_bazel_rules_go")
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.deps.len(), 2);
        assert_eq!(result.info.deps[0].name.as_str(), "rules_rust");
        assert_eq!(result.info.deps[0].version.as_str(), "0.40.0");
        assert_eq!(result.info.deps[1].name.as_str(), "rules_go");
        assert_eq!(
            result.info.deps[1].repo_name,
            Some("io_bazel_rules_go".to_string())
        );
    }

    #[test]
    fn test_parse_dev_dependency() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_rust", version = "0.40.0")
bazel_dep(name = "buildifier", version = "6.0.0", dev_dependency = True)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.deps.len(), 1);
        assert_eq!(result.info.dev_deps.len(), 1);
        assert_eq!(result.info.deps[0].name.as_str(), "rules_rust");
        assert_eq!(result.info.dev_deps[0].name.as_str(), "buildifier");
        assert!(result.info.dev_deps[0].dev_dependency);
    }

    #[test]
    fn test_parse_single_version_override() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_rust", version = "0.40.0")

single_version_override(
    module_name = "rules_rust",
    version = "0.45.0",
    registry = "https://custom.registry.example",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.overrides.len(), 1);
        match &result.info.overrides[0] {
            Override::SingleVersion {
                module,
                version,
                registry,
            } => {
                assert_eq!(module.as_str(), "rules_rust");
                assert_eq!(version.as_str(), "0.45.0");
                assert_eq!(
                    registry.as_ref().unwrap(),
                    "https://custom.registry.example"
                );
            }
            _ => panic!("Expected SingleVersion override"),
        }
    }

    #[test]
    fn test_parse_git_override() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_rust")

git_override(
    module_name = "rules_rust",
    remote = "https://github.com/bazelbuild/rules_rust.git",
    commit = "abc123def456",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.overrides.len(), 1);
        match &result.info.overrides[0] {
            Override::Git {
                module,
                remote,
                commit,
                ..
            } => {
                assert_eq!(module.as_str(), "rules_rust");
                assert_eq!(remote, "https://github.com/bazelbuild/rules_rust.git");
                assert_eq!(commit.as_ref().unwrap(), "abc123def456");
            }
            _ => panic!("Expected Git override"),
        }
    }

    #[test]
    fn test_parse_local_path_override() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "my_lib")

local_path_override(
    module_name = "my_lib",
    path = "../my_lib",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.overrides.len(), 1);
        match &result.info.overrides[0] {
            Override::LocalPath { module, path } => {
                assert_eq!(module.as_str(), "my_lib");
                assert_eq!(path, "../my_lib");
            }
            _ => panic!("Expected LocalPath override"),
        }
    }

    #[test]
    fn test_parse_archive_override() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_foo")

archive_override(
    module_name = "rules_foo",
    urls = [
        "https://example.com/rules_foo-1.0.0.tar.gz",
        "https://mirror.example.com/rules_foo-1.0.0.tar.gz",
    ],
    integrity = "sha256-abcd1234",
    strip_prefix = "rules_foo-1.0.0",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.overrides.len(), 1);
        match &result.info.overrides[0] {
            Override::Archive {
                module,
                urls,
                integrity,
                strip_prefix,
            } => {
                assert_eq!(module.as_str(), "rules_foo");
                assert_eq!(urls.len(), 2);
                assert_eq!(urls[0], "https://example.com/rules_foo-1.0.0.tar.gz");
                assert_eq!(integrity.as_ref().unwrap(), "sha256-abcd1234");
                assert_eq!(strip_prefix.as_ref().unwrap(), "rules_foo-1.0.0");
            }
            _ => panic!("Expected Archive override"),
        }
    }

    #[test]
    fn test_parse_multiple_version_override() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "protobuf", version = "3.19.0")

multiple_version_override(
    module_name = "protobuf",
    versions = ["3.19.0", "3.21.0"],
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.overrides.len(), 1);
        match &result.info.overrides[0] {
            Override::MultipleVersion {
                module,
                versions,
                registry,
            } => {
                assert_eq!(module.as_str(), "protobuf");
                assert_eq!(versions.len(), 2);
                assert_eq!(versions[0].as_str(), "3.19.0");
                assert_eq!(versions[1].as_str(), "3.21.0");
                assert!(registry.is_none());
            }
            _ => panic!("Expected MultipleVersion override"),
        }
    }

    #[test]
    fn test_parse_with_comments() {
        let content = r#"
# This is my module
module(
    name = "test",  # inline comment
    version = "1.0.0",
)

# Dependencies
bazel_dep(name = "rules_rust", version = "0.40.0")
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.name.as_str(), "test");
        assert_eq!(result.info.deps.len(), 1);
    }

    #[test]
    fn test_parse_escaped_strings() {
        let content = r#"
module(name = "test", version = "1.0.0")

local_path_override(
    module_name = "lib",
    path = "path\\with\\backslashes",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        match &result.info.overrides[0] {
            Override::LocalPath { path, .. } => {
                assert_eq!(path, "path\\with\\backslashes");
            }
            _ => panic!("Expected LocalPath override"),
        }
    }

    #[test]
    fn test_parse_single_quoted_strings() {
        let content = r#"
module(name = 'test', version = '1.0.0')

bazel_dep(name = 'rules_rust', version = '0.40.0')
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.name.as_str(), "test");
        assert_eq!(result.info.deps[0].name.as_str(), "rules_rust");
    }

    #[test]
    fn test_parse_no_module_error() {
        let content = r#"
bazel_dep(name = "rules_rust", version = "0.40.0")
"#;
        let result = ModuleFile::parse(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no module()"));
    }

    #[test]
    fn test_parse_multiline_call() {
        let content = r#"
module(
    name = "test",
    version = "1.0.0",
    bazel_compatibility = [
        ">=7.0.0",
        "<8.0.0",
    ],
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.bazel_compatibility.len(), 2);
        assert_eq!(result.info.bazel_compatibility[0], ">=7.0.0");
        assert_eq!(result.info.bazel_compatibility[1], "<8.0.0");
    }

    #[test]
    fn test_parse_complex_module() {
        let content = r#"
module(
    name = "my_workspace",
    version = "0.1.0",
    compatibility_level = 1,
    bazel_compatibility = [">=7.0.0"],
)

bazel_dep(name = "rules_rust", version = "0.40.0")
bazel_dep(name = "rules_go", version = "0.41.0")
bazel_dep(name = "rules_python", version = "0.31.0", dev_dependency = True)

single_version_override(
    module_name = "rules_rust",
    version = "0.45.0",
)

local_path_override(
    module_name = "internal_lib",
    path = "../internal_lib",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        assert_eq!(result.info.name.as_str(), "my_workspace");
        assert_eq!(result.info.version.as_str(), "0.1.0");
        assert_eq!(result.info.compatibility_level, 1);
        assert_eq!(result.info.deps.len(), 2);
        assert_eq!(result.info.dev_deps.len(), 1);
        assert_eq!(result.info.overrides.len(), 2);
    }

    #[test]
    fn test_unescape_string() {
        assert_eq!(unescape_string(r"hello\nworld"), "hello\nworld");
        assert_eq!(unescape_string(r"tab\there"), "tab\there");
        assert_eq!(unescape_string(r"back\\slash"), "back\\slash");
        assert_eq!(unescape_string(r#"quote\"here"#), "quote\"here");
        assert_eq!(unescape_string(r"normal string"), "normal string");
    }

    #[test]
    fn test_parse_git_override_with_tag() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_rust")

git_override(
    module_name = "rules_rust",
    remote = "https://github.com/bazelbuild/rules_rust.git",
    tag = "v0.45.0",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        match &result.info.overrides[0] {
            Override::Git { tag, commit, .. } => {
                assert_eq!(tag.as_ref().unwrap(), "v0.45.0");
                assert!(commit.is_none());
            }
            _ => panic!("Expected Git override"),
        }
    }

    #[test]
    fn test_parse_git_override_with_branch() {
        let content = r#"
module(name = "test", version = "1.0.0")

bazel_dep(name = "rules_rust")

git_override(
    module_name = "rules_rust",
    remote = "https://github.com/bazelbuild/rules_rust.git",
    branch = "main",
)
"#;
        let result = ModuleFile::parse(content).unwrap();
        match &result.info.overrides[0] {
            Override::Git { branch, .. } => {
                assert_eq!(branch.as_ref().unwrap(), "main");
            }
            _ => panic!("Expected Git override"),
        }
    }
}
