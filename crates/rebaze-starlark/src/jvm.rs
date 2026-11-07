//! Typed JVM and module-extension declarations.

use serde::ser::SerializeTupleStruct;
use serde::{Serialize, Serializer};

use crate::Reference;

#[derive(Serialize)]
#[serde(rename = "use_extension")]
pub struct UseExtension<'a>(pub &'a str, pub &'a str);

pub struct UseRepo {
    pub extension: Reference,
    pub repositories: Vec<String>,
}

impl Serialize for UseRepo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut call =
            serializer.serialize_tuple_struct("use_repo", self.repositories.len() + 1)?;
        call.serialize_field(&self.extension)?;
        for repository in &self.repositories {
            call.serialize_field(repository)?;
        }
        call.end()
    }
}

#[derive(Serialize)]
#[serde(rename = "maven.install")]
pub struct MavenInstall {
    pub name: String,
    pub artifacts: Vec<String>,
    pub repositories: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_file: Option<String>,
    pub aar_import_bzl_label: String,
}

#[derive(Serialize)]
#[serde(rename = "exports_files")]
pub struct ExportsFiles(pub Vec<String>);

#[derive(Serialize)]
#[serde(rename = "java_library")]
pub struct JavaLibrary {
    pub name: String,
    pub srcs: Vec<String>,
    pub resources: Vec<String>,
    pub visibility: Vec<String>,
    pub deps: Vec<String>,
    pub exports: Vec<String>,
    pub runtime_deps: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename = "java_test")]
pub struct JavaTest {
    pub name: String,
    pub srcs: Vec<String>,
    pub resources: Vec<String>,
    pub test_class: String,
    pub deps: Vec<String>,
    pub runtime_deps: Vec<String>,
}
