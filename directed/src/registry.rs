//! The registry owns node state. It is deliberately separate from [`Graph`],
//! which only stores connectivity; any number of graphs can share one registry.

use std::sync::Mutex;

use crate::stage::{DynStage, Erased, Stage};
use crate::value::Value;
use slab::Slab;

/// A stable identifier for a registered node.
pub type NodeId = usize;

/// A node's input values captured for cache comparison.
pub(crate) type InputSnapshot = Vec<Option<Value>>;

/// Cached inputs and outputs for one distinct input combination.
pub(crate) type CacheEntry = (InputSnapshot, Vec<Option<Value>>);

/// Cached inputs/outputs for a node, mirroring a [`crate::CachePolicy`].
pub(crate) enum CacheState {
    None,
    Last { inputs: InputSnapshot },
    All { entries: Vec<CacheEntry> },
}

/// A registered node: its stage (type-erased), its current outputs and cache.
///
/// Each node lives behind its own [`Mutex`] so independent nodes can be
/// evaluated concurrently. The stage is briefly taken out during evaluation so
/// the lock is never held across an `await`.
pub(crate) struct NodeCell {
    pub(crate) stage: Option<Box<dyn DynStage>>,
    pub(crate) outputs: Vec<Option<Value>>,
    pub(crate) cache: CacheState,
    pub(crate) has_run: bool,
}

impl NodeCell {
    fn new(stage: Box<dyn DynStage>) -> Self {
        let outputs = vec![None; stage.signature().outputs.len()];
        let cache = match stage.cache_policy() {
            crate::CachePolicy::None => CacheState::None,
            crate::CachePolicy::Last => CacheState::Last { inputs: Vec::new() },
            crate::CachePolicy::All => CacheState::All {
                entries: Vec::new(),
            },
        };
        Self {
            stage: Some(stage),
            outputs,
            cache,
            has_run: false,
        }
    }

    pub(crate) fn signature(&self) -> &'static crate::Signature {
        self.stage
            .as_ref()
            .expect("stage is only absent mid-evaluation")
            .signature()
    }

    pub(crate) fn cache_policy(&self) -> crate::CachePolicy {
        self.stage
            .as_ref()
            .expect("stage is only absent mid-evaluation")
            .cache_policy()
    }

    pub(crate) fn eval_strategy(&self) -> crate::EvalStrategy {
        self.stage
            .as_ref()
            .expect("stage is only absent mid-evaluation")
            .eval_strategy()
    }
}

/// Stores nodes and their state.
pub struct Registry {
    nodes: Slab<Mutex<NodeCell>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { nodes: Slab::new() }
    }

    /// Register a node using the default state of its stage.
    pub fn register<S: Stage>(&mut self) -> S::Handle
    where
        S::State: Default,
    {
        self.register_with_state::<S>(S::State::default())
    }

    /// Register a node with explicit initial state.
    pub fn register_with_state<S: Stage>(&mut self, state: S::State) -> S::Handle {
        let id = self
            .nodes
            .insert(Mutex::new(NodeCell::new(Box::new(Erased::<S> { state }))));
        S::handle(id)
    }

    /// Remove a node and drop its state.
    pub fn unregister(&mut self, id: NodeId) -> bool {
        self.nodes.try_remove(id).is_some()
    }

    /// Number of registered nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(crate) fn lock(&self, id: NodeId) -> Option<std::sync::MutexGuard<'_, NodeCell>> {
        self.nodes
            .get(id)
            .map(|cell| cell.lock().expect("node mutex poisoned"))
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
