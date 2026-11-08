//! Per-parse limits enforced by the grammar, including failed alternatives.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use chumsky::prelude::*;

pub(super) const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_NESTING: usize = 32;

/// Resource policy for borrowed CMake source. This does not limit earlier IO or
/// subsequent variable evaluation. Fields stay private to preserve bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseLimits {
    source_bytes: usize,
    nesting: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseLimitsError {
    #[error("CMake source byte limit must be between 1 and 4194304, requested {0}")]
    SourceBytes(usize),
    #[error("CMake recursive scope limit must be between 1 and 128, requested {0}")]
    Nesting(usize),
}

impl ParseLimits {
    pub fn new(source_bytes: usize, nesting: usize) -> Result<Self, ParseLimitsError> {
        if !(1..=4 * MAX_SOURCE_BYTES).contains(&source_bytes) {
            return Err(ParseLimitsError::SourceBytes(source_bytes));
        }
        if !(1..=128).contains(&nesting) {
            return Err(ParseLimitsError::Nesting(nesting));
        }
        Ok(Self {
            source_bytes,
            nesting,
        })
    }

    pub const fn source_bytes(self) -> usize {
        self.source_bytes
    }
    pub const fn nesting(self) -> usize {
        self.nesting
    }
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            source_bytes: MAX_SOURCE_BYTES,
            nesting: MAX_NESTING,
        }
    }
}

#[derive(Clone)]
pub(super) struct Budget(Rc<State>);

struct State {
    limit: usize,
    depth: Cell<usize>,
    exhausted: Cell<Option<usize>>,
}

pub(super) struct Scope(Rc<State>);

impl Drop for Scope {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get() - 1);
    }
}

impl Budget {
    pub(super) fn new(limits: ParseLimits) -> Self {
        Self(Rc::new(State {
            limit: limits.nesting,
            depth: Cell::new(0),
            exhausted: Cell::new(None),
        }))
    }

    /// `then` holds this scope while parsing its right-hand side and drops it
    /// on success, failure and backtracking. Count actual recursive grammar
    /// branches, not delimiter-looking text in quotes, comments or brackets.
    pub(super) fn enter(&self) -> impl Parser<char, Scope, Error = Simple<char>> + Clone + use<> {
        let state = Rc::clone(&self.0);
        empty().try_map(move |(), span: Range<usize>| {
            if let Some(offset) = state.exhausted.get() {
                return Err(exceeded(state.limit, offset..offset));
            }
            if state.depth.get() == state.limit {
                state.exhausted.set(Some(span.start));
                return Err(exceeded(state.limit, span));
            }
            state.depth.set(state.depth.get() + 1);
            Ok(Scope(Rc::clone(&state)))
        })
    }

    pub(super) fn exhausted(&self) -> Option<Simple<char>> {
        debug_assert_eq!(self.0.depth.get(), 0);
        self.0
            .exhausted
            .get()
            .map(|offset| exceeded(self.0.limit, offset..offset))
    }
}

fn exceeded(limit: usize, span: Range<usize>) -> Simple<char> {
    Simple::custom(
        span,
        format!("CMake syntax nesting budget exceeded ({limit})"),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn failed_alternative_releases_scopes_before_retrying_the_same_input() {
        let budget = Budget::new(ParseLimits::new(64, 2).unwrap());
        let fail = budget
            .enter()
            .ignore_then(budget.enter().ignore_then(just('a').then_ignore(just('z'))));
        let retry = budget
            .enter()
            .ignore_then(budget.enter().ignore_then(just('a')));
        assert_eq!(fail.or(retry).then_ignore(end()).parse("a").unwrap(), 'a');
        assert_eq!(budget.0.depth.get(), 0);
        assert!(budget.exhausted().is_none());
    }
}
