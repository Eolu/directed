//! Error types for the crate.

use crate::{BuildError, NodeId};

/// Errors produced while a stage reads its inputs or writes its outputs.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("missing input `{name}` (index {index})")]
    MissingInput { index: usize, name: &'static str },

    #[error("input `{name}` (index {index}) type mismatch, expected `{expected}`")]
    InputTypeMismatch {
        index: usize,
        name: &'static str,
        expected: &'static str,
    },

    #[error(
        "input `{name}` (index {index}) is shared but must be moved; `{expected}` is not `Clone`"
    )]
    SharedInput {
        index: usize,
        name: &'static str,
        expected: &'static str,
    },
}

/// Errors produced by the execution engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("stage `{stage}` (node {node}) failed: {source}")]
    Node {
        node: NodeId,
        stage: &'static str,
        #[source]
        source: CallError,
    },

    #[error("node {0} is not registered")]
    UnknownNode(NodeId),

    #[error("output port index {index} of node {node} has no value")]
    MissingOutput { node: NodeId, index: u16 },

    #[error("execution task failed to join: {0}")]
    TaskJoin(String),

    #[error(transparent)]
    Wiring(#[from] BuildError),
}
