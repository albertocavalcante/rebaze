const MIB: usize = 1024 * 1024;

/// Caller-selected intent, separate from actual observed usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    pub(super) file_bytes: usize,
    pub(super) total_bytes: usize,
    pub(super) files: usize,
    pub(super) operations: usize,
    pub(super) depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReadLimitsError {
    #[error("{field} must be between 1 and {max}, requested {requested}")]
    Range {
        field: &'static str,
        requested: usize,
        max: usize,
    },
    #[error("aggregate bytes must be at least the per-file byte limit")]
    AggregateBelowFile,
}

impl ReadLimits {
    pub fn new(
        file_bytes: usize,
        total_bytes: usize,
        files: usize,
    ) -> Result<Self, ReadLimitsError> {
        check("file bytes", file_bytes, 64 * MIB)?;
        check("aggregate bytes", total_bytes, 512 * MIB)?;
        check("file attempts", files, 65_536)?;
        if total_bytes < file_bytes {
            return Err(ReadLimitsError::AggregateBelowFile);
        }
        Ok(Self {
            file_bytes,
            total_bytes,
            files,
            ..Self::default()
        })
    }

    /// Operations count public lookups/reads and read attempts,
    /// including failures. They are not exact underlying syscall counts.
    pub fn with_traversal(
        mut self,
        operations: usize,
        depth: usize,
    ) -> Result<Self, ReadLimitsError> {
        check("logical operations", operations, 1_048_576)?;
        check("relative path depth", depth, 128)?;
        self.operations = operations;
        self.depth = depth;
        Ok(self)
    }

    pub const fn file_bytes(self) -> usize {
        self.file_bytes
    }
    pub const fn total_bytes(self) -> usize {
        self.total_bytes
    }
    pub const fn file_attempts(self) -> usize {
        self.files
    }
    pub const fn operations(self) -> usize {
        self.operations
    }
    pub const fn depth(self) -> usize {
        self.depth
    }
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            file_bytes: MIB,
            total_bytes: 64 * MIB,
            files: 4096,
            operations: 65_536,
            depth: 64,
        }
    }
}

fn check(field: &'static str, requested: usize, max: usize) -> Result<(), ReadLimitsError> {
    if !(1..=max).contains(&requested) {
        return Err(ReadLimitsError::Range {
            field,
            requested,
            max,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    FileBytes,
    AggregateBytes,
    FileAttempts,
    Operations,
    PathDepth,
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FileBytes => "file bytes",
            Self::AggregateBytes => "aggregate bytes",
            Self::FileAttempts => "file attempts",
            Self::Operations => "logical operations",
            Self::PathDepth => "relative path depth",
        })
    }
}

/// Actual charged work. Oversize reads include their single proof byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub(super) bytes: usize,
    pub(super) files: usize,
    pub(super) operations: usize,
}

impl Usage {
    pub const fn bytes(self) -> usize {
        self.bytes
    }
    pub const fn file_attempts(self) -> usize {
        self.files
    }
    pub const fn operations(self) -> usize {
        self.operations
    }
}
