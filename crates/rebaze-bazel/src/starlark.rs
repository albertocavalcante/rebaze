//! Type-safe Starlark/Bazel rule definitions using serde_starlark.
//!
//! This module provides Rust types that serialize to valid Starlark code via serde_starlark.
//! Use these types instead of manual string formatting for type-safe BUILD file generation.

use serde::ser::{SerializeStruct, SerializeTupleStruct, Serializer};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Load statement: load("@rules_cc//cc:defs.bzl", "cc_binary", "cc_library")
pub struct Load {
    pub bzl: String,
    pub items: BTreeSet<String>,
}

impl Serialize for Load {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_tuple_struct("load", 0)?;
        s.serialize_field(&self.bzl)?;
        for item in &self.items {
            s.serialize_field(item)?;
        }
        s.end()
    }
}

/// Package visibility
pub struct Package {
    pub default_visibility: Vec<String>,
}

impl Serialize for Package {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("package", 1)?;
        s.serialize_field("default_visibility", &self.default_visibility)?;
        s.end()
    }
}

/// MODULE.bazel module() declaration
#[derive(Serialize)]
#[serde(rename = "module")]
pub struct Module {
    pub name: String,
    pub version: String,
}

/// bazel_dep declaration
#[derive(Serialize)]
#[serde(rename = "bazel_dep")]
pub struct BazelDep {
    pub name: String,
    pub version: String,
}

/// cc_library rule
#[derive(Serialize, Default)]
#[serde(rename = "cc_library")]
pub struct CcLibrary {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdrs: Option<Glob>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub copts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linkopts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linkshared: Option<bool>,
}

/// Bazel alias rule
#[derive(Serialize)]
#[serde(rename = "alias")]
pub struct Alias {
    pub name: String,
    pub actual: FunctionCall,
}

/// A function call expression like resolve_dep("glib")
/// Serializes to: resolve_dep("glib")
pub struct FunctionCall {
    pub name: &'static str,
    pub args: Vec<String>,
}

impl Serialize for FunctionCall {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeTupleStruct;
        let mut s = serializer.serialize_tuple_struct(self.name, self.args.len())?;
        for arg in &self.args {
            s.serialize_field(arg)?;
        }
        s.end()
    }
}

/// cc_binary rule
#[derive(Serialize)]
#[serde(rename = "cc_binary")]
pub struct CcBinary {
    pub name: String,
    pub srcs: SrcsWithHdrs,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub copts: Vec<String>,
}

/// Source files with optional header glob patterns.
/// Serializes as: ["file1.c", "file2.c"] + glob(["**/*.h"]) if hdrs_glob is Some,
/// or just ["file1.c", "file2.c"] if hdrs_glob is None.
pub struct SrcsWithHdrs {
    pub files: Vec<String>,
    pub hdrs_glob: Option<Vec<String>>,
}

impl Serialize for SrcsWithHdrs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.hdrs_glob {
            None => {
                // Just serialize as a plain list
                self.files.serialize(serializer)
            }
            Some(patterns) => {
                // Serialize as files list + glob with allow_empty=True
                // allow_empty is needed since not all C/C++ projects have both .h and .hpp files
                let glob = Glob {
                    include: patterns.clone(),
                    exclude: Vec::new(),
                    allow_empty: true,
                };
                let add = Add {
                    lhs: &self.files,
                    rhs: &glob,
                };
                add.serialize(serializer)
            }
        }
    }
}

/// Addition expression: lhs + rhs
struct Add<'a, L, R> {
    lhs: &'a L,
    rhs: &'a R,
}

impl<L: Serialize, R: Serialize> Serialize for Add<'_, L, R> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_tuple_struct("+", 2)?;
        s.serialize_field(self.lhs)?;
        s.serialize_field(self.rhs)?;
        s.end()
    }
}

/// Glob pattern
pub struct Glob {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub allow_empty: bool,
}

impl Glob {
    /// Create a new glob with the given include patterns, allowing empty results.
    pub fn new_allow_empty(include: Vec<String>) -> Self {
        Self {
            include,
            exclude: Vec::new(),
            allow_empty: true,
        }
    }
}

impl Serialize for Glob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.exclude.is_empty() && !self.allow_empty {
            serializer.serialize_newtype_struct("glob", &self.include)
        } else {
            let mut s = serializer.serialize_struct("glob", 3)?;
            s.serialize_field("include", &self.include)?;
            if !self.exclude.is_empty() {
                s.serialize_field("exclude", &self.exclude)?;
            }
            if self.allow_empty {
                s.serialize_field("allow_empty", &true)?;
            }
            s.end()
        }
    }
}

// =============================================================================
// rules_foreign_cc rules
// =============================================================================

/// rules_foreign_cc cmake() rule for building CMake projects.
#[derive(Serialize, Default)]
#[serde(rename = "cmake")]
pub struct Cmake {
    pub name: String,
    pub lib_source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out_static_libs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out_shared_libs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub generate_args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cache_entries: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Vec<String>>,
}

impl Cmake {
    /// Create a new cmake rule with Ninja generator.
    pub fn new(name: impl Into<String>, lib_source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lib_source: lib_source.into(),
            generate_args: vec!["-GNinja".to_string()],
            ..Default::default()
        }
    }

    pub fn with_static_libs(mut self, libs: Vec<impl Into<String>>) -> Self {
        self.out_static_libs = libs.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_deps(mut self, deps: Vec<impl Into<String>>) -> Self {
        self.deps = deps.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_visibility_public(mut self) -> Self {
        self.visibility = Some(vec!["//visibility:public".to_string()]);
        self
    }
}

/// rules_foreign_cc meson() rule for building Meson projects.
#[derive(Serialize, Default)]
#[serde(rename = "meson")]
pub struct Meson {
    pub name: String,
    pub lib_source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out_static_libs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out_shared_libs: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Vec<String>>,
}

impl Meson {
    pub fn new(name: impl Into<String>, lib_source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lib_source: lib_source.into(),
            ..Default::default()
        }
    }

    pub fn with_static_libs(mut self, libs: Vec<impl Into<String>>) -> Self {
        self.out_static_libs = libs.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_options(mut self, options: BTreeMap<String, String>) -> Self {
        self.options = options;
        self
    }

    pub fn with_deps(mut self, deps: Vec<impl Into<String>>) -> Self {
        self.deps = deps.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_visibility_public(mut self) -> Self {
        self.visibility = Some(vec!["//visibility:public".to_string()]);
        self
    }
}

/// rules_foreign_cc configure_make() rule for building autotools projects.
#[derive(Serialize, Default)]
#[serde(rename = "configure_make")]
pub struct ConfigureMake {
    pub name: String,
    pub lib_source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out_static_libs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub out_shared_libs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub configure_options: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Vec<String>>,
}

impl ConfigureMake {
    pub fn new(name: impl Into<String>, lib_source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lib_source: lib_source.into(),
            ..Default::default()
        }
    }

    pub fn with_static_libs(mut self, libs: Vec<impl Into<String>>) -> Self {
        self.out_static_libs = libs.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_deps(mut self, deps: Vec<impl Into<String>>) -> Self {
        self.deps = deps.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_visibility_public(mut self) -> Self {
        self.visibility = Some(vec!["//visibility:public".to_string()]);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmake_serialization() {
        let cmake = Cmake::new("zlib_source", "@zlib_src//:all")
            .with_static_libs(vec!["libz.a"])
            .with_visibility_public();
        let output = serde_starlark::to_string(&cmake).unwrap();
        assert!(output.contains("cmake("));
        assert!(output.contains("name = \"zlib_source\""));
        assert!(output.contains("lib_source = \"@zlib_src//:all\""));
        assert!(output.contains("out_static_libs = ["));
        assert!(output.contains("-GNinja"));
    }

    #[test]
    fn test_meson_serialization() {
        let mut options = BTreeMap::new();
        options.insert("tests".to_string(), "false".to_string());
        options.insert("introspection".to_string(), "disabled".to_string());

        let meson = Meson::new("glib_source", "@glib_src//:all")
            .with_static_libs(vec!["libglib-2.0.a"])
            .with_options(options)
            .with_deps(vec![":pcre2_source", ":zlib_source"])
            .with_visibility_public();
        let output = serde_starlark::to_string(&meson).unwrap();
        assert!(output.contains("meson("));
        assert!(output.contains("options = {"));
        assert!(output.contains("deps = ["));
    }

    #[test]
    fn test_configure_make_serialization() {
        let rule = ConfigureMake::new("libffi_source", "@libffi_src//:all")
            .with_static_libs(vec!["libffi.a"])
            .with_visibility_public();
        let output = serde_starlark::to_string(&rule).unwrap();
        assert!(output.contains("configure_make("));
        assert!(output.contains("out_static_libs = ["));
    }
}
