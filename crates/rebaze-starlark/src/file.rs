//! Structured file assembly around the Serde Starlark serializer.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::Load;

/// A generation failure, before any output is written to disk.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid Starlark identifier: {0:?}")]
    InvalidIdentifier(String),
    #[error("failed to serialize Starlark: {0}")]
    Serialize(#[from] serde_starlark::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A variable binding, distinct from a quoted string or a Bazel label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identifier(String);

impl Identifier {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if valid_identifier(&name) {
            Ok(Self(name))
        } else {
            Err(Error::InvalidIdentifier(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn valid_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !matches!(
            name,
            "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "load"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
                | "True"
                | "False"
                | "None"
        )
}

pub fn validate_function(name: &str) -> Result<()> {
    if name.split('.').all(valid_identifier) {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier(name.into()))
    }
}

/// A reference to a variable defined by generator code, never project data.
#[derive(Debug, Clone, Copy)]
pub struct Reference(&'static str);

impl Reference {
    pub fn new(name: &'static str) -> Result<Self> {
        Identifier::new(name)?;
        Ok(Self(name))
    }
}

impl Serialize for Reference {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_unit_struct(self.0)
    }
}

/// A named call with positional or keyword arguments represented by Serde types.
pub struct Call<A> {
    function: String,
    arguments: A,
}

impl<A> Call<A> {
    pub fn new(function: impl Into<String>, arguments: A) -> Result<Self> {
        let function = function.into();
        validate_function(&function)?;
        Ok(Self {
            function,
            arguments,
        })
    }
}

impl<A: Serialize> Serialize for Call<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serde_starlark::FunctionCall::new(&self.function, &self.arguments).serialize(serializer)
    }
}

/// A file with an explicit boundary between serialized data and trusted helper code.
/// Loads are combined and placed before the body; statement order is preserved.
#[derive(Default)]
pub struct File {
    header: Vec<String>,
    loads: BTreeMap<String, BTreeSet<String>>,
    body: Vec<String>,
}

impl File {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn docstring(&mut self, text: &str) -> Result<()> {
        self.header.push(serde_starlark::to_string(text)?);
        Ok(())
    }

    /// Prefix every line, including carriage-return-separated lines, as a comment.
    pub fn comment(&mut self, text: &str) {
        self.body.push(
            text.split(['\r', '\n'])
                .map(|line| format!("# {}", line.escape_debug()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    pub fn load(&mut self, load: Load) -> Result<()> {
        for item in &load.items {
            Identifier::new(item.clone())?;
        }
        self.loads.entry(load.bzl).or_default().extend(load.items);
        Ok(())
    }

    pub fn statement<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let source = serde_starlark::to_string(value)?;
        self.body.push(source);
        Ok(())
    }

    pub fn assign<T: Serialize>(&mut self, name: &Identifier, value: &T) -> Result<()> {
        self.statement(&serde_starlark::Assignment::new(name.as_str(), value))
    }

    pub fn commented_statement<T: Serialize>(&mut self, value: &T) -> Result<()> {
        let source = serde_starlark::to_string(value)?;
        self.body.push(
            source
                .lines()
                .map(|line| format!("# {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        Ok(())
    }

    /// Include reviewed, static Starlark source. Dynamic values belong in `assign`
    /// or `statement`; this method deliberately accepts only static source.
    pub fn helper_source(&mut self, source: &'static str) {
        self.body.push(source.trim().to_owned());
    }

    pub fn append(&mut self, other: Self) {
        self.header.extend(other.header);
        for (path, items) in other.loads {
            self.loads.entry(path).or_default().extend(items);
        }
        self.body.extend(other.body);
    }

    pub fn render(self) -> Result<String> {
        let mut sections = self.header;
        for (bzl, items) in self.loads {
            sections.push(serde_starlark::to_string(&Load { bzl, items })?);
        }
        sections.extend(self.body);
        Ok(sections.join("\n\n") + "\n")
    }
}
