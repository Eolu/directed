//! Static, per-stage descriptions of input and output ports.

use crate::value::ValueOps;

/// How a stage receives an input: by value or by reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Owned,
    Borrowed,
    /// A `&mut T` input. The value is copied into the node and mutated locally;
    /// changes are not visible to the producing node.
    BorrowedMut,
}

/// A single input port of a stage.
#[derive(Clone, Copy)]
pub struct InputPort {
    pub name: &'static str,
    pub ref_kind: RefKind,
    pub ops: ValueOps,
}

/// A single output port of a stage.
#[derive(Clone, Copy)]
pub struct OutputPort {
    pub name: &'static str,
    pub ops: ValueOps,
}

impl InputPort {
    pub fn type_id(&self) -> std::any::TypeId {
        self.ops.type_id
    }

    pub fn type_name(&self) -> &'static str {
        self.ops.type_name
    }
}

impl OutputPort {
    pub fn type_id(&self) -> std::any::TypeId {
        self.ops.type_id
    }

    pub fn type_name(&self) -> &'static str {
        self.ops.type_name
    }
}

/// The full static signature of a stage. Built once and leaked.
pub struct Signature {
    pub stage: &'static str,
    pub inputs: Vec<InputPort>,
    pub outputs: Vec<OutputPort>,
}
