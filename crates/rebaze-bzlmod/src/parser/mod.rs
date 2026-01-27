//! MODULE.bazel file parsing.
//!
//! This module provides a parser for MODULE.bazel files using the Starlark AST.
//! It extracts module declarations, dependencies, and overrides.

use crate::{Dependency, ModuleInfo, Override, Result};
use starlark::syntax::{AstModule, Dialect};
use starlark_syntax::codemap::Spanned;
use starlark_syntax::syntax::ast::{ArgumentP, AstLiteral, AstNoPayload, CallArgsP, ExprP, StmtP};
use starlark_syntax::syntax::module::AstModuleFields;

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

impl ModuleFile {
    /// Parse `MODULE.bazel` content.
    ///
    /// # Errors
    ///
    /// Returns an error if the content is invalid or missing required declarations.
    pub fn parse(content: &str) -> Result<Self> {
        // Use Extended dialect to support all Bazel MODULE.bazel constructs
        let dialect = Dialect::Extended;
        let ast = AstModule::parse("MODULE.bazel", content.to_owned(), &dialect)
            .map_err(|e| crate::Error::Parse(e.to_string()))?;

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

        // Get the AST parts - statement is the top-level statement
        let (_, statement, _, _) = ast.into_parts();

        // Visit all top-level statements
        visit_stmt(&statement, &mut info, &mut found_module)?;

        if !found_module {
            return Err(crate::Error::Parse(
                "no module() declaration found".to_string(),
            ));
        }

        Ok(Self {
            info,
            content: content.to_string(),
        })
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

/// Visit a statement and extract module information.
fn visit_stmt(
    stmt: &Spanned<StmtP<AstNoPayload>>,
    info: &mut ModuleInfo,
    found_module: &mut bool,
) -> Result<()> {
    match &stmt.node {
        StmtP::Statements(stmts) => {
            for s in stmts {
                visit_stmt(s, info, found_module)?;
            }
        }
        StmtP::Expression(expr) => {
            visit_expr(expr, info, found_module)?;
        }
        // Ignore other statement types (load, def, if, for, etc.)
        _ => {}
    }
    Ok(())
}

/// Visit an expression and extract function calls.
fn visit_expr(
    expr: &Spanned<ExprP<AstNoPayload>>,
    info: &mut ModuleInfo,
    found_module: &mut bool,
) -> Result<()> {
    if let ExprP::Call(func_expr, args) = &expr.node {
        // Get the function name
        if let Some(name) = get_func_name(func_expr) {
            match name.as_str() {
                "module" => {
                    *found_module = true;
                    parse_module(args, info)?;
                }
                "bazel_dep" => {
                    if let Some(dep) = parse_bazel_dep(args)? {
                        if dep.dev_dependency {
                            info.dev_deps.push(dep);
                        } else {
                            info.deps.push(dep);
                        }
                    }
                }
                "single_version_override" => {
                    if let Some(ov) = parse_single_version_override(args)? {
                        info.overrides.push(ov);
                    }
                }
                "git_override" => {
                    if let Some(ov) = parse_git_override(args)? {
                        info.overrides.push(ov);
                    }
                }
                "local_path_override" => {
                    if let Some(ov) = parse_local_path_override(args)? {
                        info.overrides.push(ov);
                    }
                }
                "archive_override" => {
                    if let Some(ov) = parse_archive_override(args)? {
                        info.overrides.push(ov);
                    }
                }
                "multiple_version_override" => {
                    if let Some(ov) = parse_multiple_version_override(args)? {
                        info.overrides.push(ov);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Get the function name from a call expression.
fn get_func_name(expr: &Spanned<ExprP<AstNoPayload>>) -> Option<String> {
    match &expr.node {
        ExprP::Identifier(ident) => Some(ident.node.ident.clone()),
        _ => None,
    }
}

/// Extract a named string argument from call arguments.
fn get_string_arg(args: &CallArgsP<AstNoPayload>, name: &str) -> Option<String> {
    for arg in &args.args {
        if let ArgumentP::Named(arg_name, value) = &arg.node {
            if arg_name.node == name {
                return extract_string(value);
            }
        }
    }
    None
}

/// Extract a named integer argument from call arguments.
fn get_int_arg(args: &CallArgsP<AstNoPayload>, name: &str) -> Option<i64> {
    for arg in &args.args {
        if let ArgumentP::Named(arg_name, value) = &arg.node {
            if arg_name.node == name {
                return extract_int(value);
            }
        }
    }
    None
}

/// Extract a named boolean argument from call arguments.
fn get_bool_arg(args: &CallArgsP<AstNoPayload>, name: &str) -> Option<bool> {
    for arg in &args.args {
        if let ArgumentP::Named(arg_name, value) = &arg.node {
            if arg_name.node == name {
                return extract_bool(value);
            }
        }
    }
    None
}

/// Extract a named string list argument from call arguments.
fn get_string_list_arg(args: &CallArgsP<AstNoPayload>, name: &str) -> Option<Vec<String>> {
    for arg in &args.args {
        if let ArgumentP::Named(arg_name, value) = &arg.node {
            if arg_name.node == name {
                return extract_string_list(value);
            }
        }
    }
    None
}

/// Extract a string value from an expression.
fn extract_string(expr: &Spanned<ExprP<AstNoPayload>>) -> Option<String> {
    if let ExprP::Literal(AstLiteral::String(s)) = &expr.node {
        return Some(s.node.clone());
    }
    None
}

/// Extract an integer value from an expression.
fn extract_int(expr: &Spanned<ExprP<AstNoPayload>>) -> Option<i64> {
    if let ExprP::Literal(AstLiteral::Int(i)) = &expr.node {
        // TokenInt has different variants, we need to extract the value
        // by converting to string and parsing
        let s = format!("{}", i.node);
        return s.parse().ok();
    }
    // Also handle unary minus for negative numbers
    if let ExprP::Minus(inner) = &expr.node {
        if let Some(val) = extract_int(inner) {
            return Some(-val);
        }
    }
    None
}

/// Extract a boolean value from an expression.
fn extract_bool(expr: &Spanned<ExprP<AstNoPayload>>) -> Option<bool> {
    if let ExprP::Identifier(ident) = &expr.node {
        match ident.node.ident.as_str() {
            "True" => return Some(true),
            "False" => return Some(false),
            _ => {}
        }
    }
    None
}

/// Extract a string list from an expression.
fn extract_string_list(expr: &Spanned<ExprP<AstNoPayload>>) -> Option<Vec<String>> {
    if let ExprP::List(items) = &expr.node {
        let mut result = Vec::new();
        for item in items {
            if let Some(s) = extract_string(item) {
                result.push(s);
            }
        }
        return Some(result);
    }
    None
}

/// Parse `module()` declaration.
fn parse_module(args: &CallArgsP<AstNoPayload>, info: &mut ModuleInfo) -> Result<()> {
    if let Some(name) = get_string_arg(args, "name") {
        info.name = crate::label::ModuleName::new(name)?;
    }

    if let Some(version) = get_string_arg(args, "version") {
        info.version = crate::label::Version::new(version)?;
    }

    if let Some(level) = get_int_arg(args, "compatibility_level") {
        info.compatibility_level = u32::try_from(level).unwrap_or(0);
    }

    if let Some(compat) = get_string_list_arg(args, "bazel_compatibility") {
        info.bazel_compatibility = compat;
    }

    Ok(())
}

/// Parse `bazel_dep()` declaration.
fn parse_bazel_dep(args: &CallArgsP<AstNoPayload>) -> Result<Option<Dependency>> {
    let name = match get_string_arg(args, "name") {
        Some(n) => crate::label::ModuleName::new(n)?,
        None => {
            return Err(crate::Error::Parse(
                "bazel_dep requires 'name' attribute".to_string(),
            ));
        }
    };

    // Version is optional when using overrides
    let version = match get_string_arg(args, "version") {
        Some(v) => crate::label::Version::new(v)?,
        None => crate::label::Version::new("0.0.0")?,
    };

    let max_version = get_string_arg(args, "max_version")
        .map(crate::label::Version::new)
        .transpose()?;

    let repo_name = get_string_arg(args, "repo_name");
    let dev_dependency = get_bool_arg(args, "dev_dependency").unwrap_or(false);

    Ok(Some(Dependency {
        name,
        version,
        max_version,
        repo_name,
        dev_dependency,
    }))
}

/// Parse `single_version_override()` declaration.
fn parse_single_version_override(args: &CallArgsP<AstNoPayload>) -> Result<Option<Override>> {
    let module_name = match get_string_arg(args, "module_name") {
        Some(n) => crate::label::ModuleName::new(n)?,
        None => return Ok(None),
    };

    let version = match get_string_arg(args, "version") {
        Some(v) => crate::label::Version::new(v)?,
        None => crate::label::Version::new("0.0.0")?,
    };

    let registry = get_string_arg(args, "registry");

    Ok(Some(Override::SingleVersion {
        module: module_name,
        version,
        registry,
    }))
}

/// Parse `git_override()` declaration.
fn parse_git_override(args: &CallArgsP<AstNoPayload>) -> Result<Option<Override>> {
    let module_name = match get_string_arg(args, "module_name") {
        Some(n) => crate::label::ModuleName::new(n)?,
        None => return Ok(None),
    };

    let remote = get_string_arg(args, "remote").unwrap_or_default();
    let commit = get_string_arg(args, "commit");
    let tag = get_string_arg(args, "tag");
    let branch = get_string_arg(args, "branch");

    Ok(Some(Override::Git {
        module: module_name,
        remote,
        commit,
        tag,
        branch,
    }))
}

/// Parse `local_path_override()` declaration.
fn parse_local_path_override(args: &CallArgsP<AstNoPayload>) -> Result<Option<Override>> {
    let module_name = match get_string_arg(args, "module_name") {
        Some(n) => crate::label::ModuleName::new(n)?,
        None => return Ok(None),
    };

    let path = get_string_arg(args, "path").unwrap_or_default();

    Ok(Some(Override::LocalPath {
        module: module_name,
        path,
    }))
}

/// Parse `archive_override()` declaration.
fn parse_archive_override(args: &CallArgsP<AstNoPayload>) -> Result<Option<Override>> {
    let module_name = match get_string_arg(args, "module_name") {
        Some(n) => crate::label::ModuleName::new(n)?,
        None => return Ok(None),
    };

    let urls = get_string_list_arg(args, "urls").unwrap_or_default();
    let integrity = get_string_arg(args, "integrity");
    let strip_prefix = get_string_arg(args, "strip_prefix");

    Ok(Some(Override::Archive {
        module: module_name,
        urls,
        integrity,
        strip_prefix,
    }))
}

/// Parse `multiple_version_override()` declaration.
fn parse_multiple_version_override(args: &CallArgsP<AstNoPayload>) -> Result<Option<Override>> {
    let module_name = match get_string_arg(args, "module_name") {
        Some(n) => crate::label::ModuleName::new(n)?,
        None => return Ok(None),
    };

    let version_strings = get_string_list_arg(args, "versions").unwrap_or_default();
    let mut versions = Vec::new();
    for v in version_strings {
        versions.push(crate::label::Version::new(v)?);
    }

    let registry = get_string_arg(args, "registry");

    Ok(Some(Override::MultipleVersion {
        module: module_name,
        versions,
        registry,
    }))
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
    fn test_parse_single_quoted_strings() {
        let content = r"
module(name = 'test', version = '1.0.0')

bazel_dep(name = 'rules_rust', version = '0.40.0')
";
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
