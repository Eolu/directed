//! `directed` is a directed-acyclic-graph execution engine. Functions wrapped
//! with [`macro@stage`] become statically-typed node definitions; registering
//! one with a [`Registry`] yields a typed handle, and a [`Graph`] wires handles
//! together. Executing a graph walks to the nearest urgent nodes, reusing
//! cached results where the input has not changed.
//!
//! Graphs are stateless and hold only connectivity, so any number of graphs can
//! share a single registry, and connections can be rewired at runtime without
//! losing node state.

mod diagnostics;
mod error;
mod execute;
mod graph;
mod io;
mod registry;
mod signature;
mod stage;
mod value;

pub use directed_stage_macro::stage;

pub use diagnostics::{Trace, TraceEdge, TraceNode};
pub use error::{CallError, Error};
pub use execute::{Outputs, block_on};
pub use graph::{BuildError, Edge, Graph, GraphBuilder, PortIn, PortOut};
pub use io::Io;
pub use registry::{NodeId, Registry};
pub use signature::{InputPort, OutputPort, RefKind, Signature, intern_signature};
pub use stage::{CachePolicy, EvalStrategy, Stage, StageHandle};
pub use value::{Value, ValueOps};
