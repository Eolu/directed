//! The async execution engine.
//!
//! Nodes are evaluated in dependency order. Independent nodes run concurrently
//! on the current thread; with the `tokio` feature, [`Graph::execute_tokio`]
//! spawns each ready node onto a multi-thread runtime. A node runs when it is
//! opaque or when its inputs changed since the last evaluation.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use futures::stream::{FuturesUnordered, StreamExt};

use crate::error::Error;
use crate::graph::{Edge, Graph};
use crate::io::Io;
use crate::registry::{CacheState, NodeId, Registry};
use crate::signature::Signature;
use crate::stage::{CachePolicy, EvalStrategy};
use crate::value::Value;

/// Values produced by a graph execution, keyed by node and output index.
#[derive(Default)]
pub struct Outputs {
    values: HashMap<(NodeId, u16), Value>,
}

impl Outputs {
    pub fn get<T: 'static>(&self, node: NodeId, index: u16) -> Option<&T> {
        self.values
            .get(&(node, index))
            .and_then(|value| value.downcast_ref::<T>())
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl Graph {
    /// Run every urgent node (and its ancestors) to completion.
    pub fn execute(&self, registry: &Registry) -> Result<Outputs, Error> {
        block_on(self.execute_async(registry, &self.urgent_targets(registry)))
    }

    /// Run the given target nodes (and their ancestors), evaluating independent
    /// nodes concurrently on the current thread.
    pub async fn execute_async(
        &self,
        registry: &Registry,
        targets: &[NodeId],
    ) -> Result<Outputs, Error> {
        let mut schedule = Schedule::new(self, targets);
        let mut ready = schedule.ready();
        let mut remaining = schedule.len();
        let mut running = FuturesUnordered::new();

        while remaining > 0 {
            for node in ready.drain(..) {
                let incoming = self.incoming_for(node);
                running.push(async move { (node, eval_node(registry, node, incoming).await) });
            }
            match running.next().await {
                Some((node, result)) => {
                    result?;
                    remaining -= 1;
                    schedule.advance(node, &mut ready);
                }
                None => break,
            }
        }

        Ok(self.collect_outputs(registry, targets))
    }

    /// Run the given target nodes on a multi-thread Tokio runtime.
    ///
    /// Requires the `tokio` feature and an `Arc<Registry>` so tasks can own a
    /// handle to the node state.
    #[cfg(feature = "tokio")]
    pub async fn execute_tokio(
        &self,
        registry: std::sync::Arc<Registry>,
        targets: &[NodeId],
    ) -> Result<Outputs, Error> {
        let mut schedule = Schedule::new(self, targets);
        let mut ready = schedule.ready();
        let mut remaining = schedule.len();
        let mut running = tokio::task::JoinSet::new();

        while remaining > 0 {
            for node in ready.drain(..) {
                let registry = registry.clone();
                let incoming = self.incoming_for(node);
                running.spawn(async move { (node, eval_node(&registry, node, incoming).await) });
            }
            match running.join_next().await {
                Some(Ok((node, result))) => {
                    result?;
                    remaining -= 1;
                    schedule.advance(node, &mut ready);
                }
                Some(Err(join_error)) => {
                    return Err(Error::TaskJoin(join_error.to_string()));
                }
                None => break,
            }
        }

        Ok(self.collect_outputs(&registry, targets))
    }

    fn urgent_targets(&self, registry: &Registry) -> Vec<NodeId> {
        self.nodes()
            .iter()
            .copied()
            .filter(|id| {
                registry
                    .lock(*id)
                    .map(|node| node.eval_strategy() == EvalStrategy::Urgent)
                    .unwrap_or(false)
            })
            .collect()
    }

    fn incoming_for(&self, node: NodeId) -> Vec<Edge> {
        self.incoming_edges(node)
            .iter()
            .map(|&edge_index| self.edges()[edge_index])
            .collect()
    }

    fn collect_outputs(&self, registry: &Registry, targets: &[NodeId]) -> Outputs {
        let mut values = HashMap::new();
        for &id in targets {
            if let Some(node) = registry.lock(id) {
                for (index, value) in node.outputs.iter().enumerate() {
                    if let Some(value) = value {
                        values.insert((id, index as u16), value.clone());
                    }
                }
            }
        }
        Outputs { values }
    }
}

/// Dependency bookkeeping for one execution: in-degrees and children, limited
/// to the ancestor closure of the targets.
struct Schedule {
    in_degree: HashMap<NodeId, usize>,
    children: HashMap<NodeId, Vec<NodeId>>,
}

impl Schedule {
    fn new(graph: &Graph, targets: &[NodeId]) -> Self {
        let mut nodes = HashSet::new();
        let mut stack = targets.to_vec();
        while let Some(node) = stack.pop() {
            if nodes.insert(node) {
                for edge_index in graph.incoming_edges(node) {
                    stack.push(graph.edges()[*edge_index].from);
                }
            }
        }

        let mut in_degree: HashMap<NodeId, usize> = nodes.iter().map(|&node| (node, 0)).collect();
        let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for &node in &nodes {
            for edge_index in graph.incoming_edges(node) {
                let parent = graph.edges()[*edge_index].from;
                if nodes.contains(&parent) {
                    *in_degree.get_mut(&node).expect("present") += 1;
                    children.entry(parent).or_default().push(node);
                }
            }
        }

        Self {
            in_degree,
            children,
        }
    }

    fn len(&self) -> usize {
        self.in_degree.len()
    }

    fn ready(&self) -> Vec<NodeId> {
        self.in_degree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(&node, _)| node)
            .collect()
    }

    fn advance(&mut self, node: NodeId, ready: &mut Vec<NodeId>) {
        if let Some(children) = self.children.get(&node) {
            for &child in children {
                let degree = self.in_degree.get_mut(&child).expect("child is scheduled");
                *degree -= 1;
                if *degree == 0 {
                    ready.push(child);
                }
            }
        }
    }
}

/// Evaluate one node: gather inputs from parents, consult the cache, run the
/// stage, and store outputs.
async fn eval_node(registry: &Registry, id: NodeId, incoming: Vec<Edge>) -> Result<(), Error> {
    let (signature, cache_enabled): (&'static Signature, bool) = {
        let node = registry.lock(id).ok_or(Error::UnknownNode(id))?;
        (node.signature(), node.cache_policy() != CachePolicy::None)
    };

    let mut gathered: Vec<Option<Value>> = vec![None; signature.inputs.len()];
    for edge in incoming {
        let value = {
            let parent = registry
                .lock(edge.from)
                .ok_or(Error::UnknownNode(edge.from))?;
            parent
                .outputs
                .get(edge.from_index as usize)
                .and_then(|slot| slot.as_ref())
                .ok_or(Error::MissingOutput {
                    node: edge.from,
                    index: edge.from_index,
                })?
                .clone()
        };
        gathered[edge.to_index as usize] = Some(value);
    }

    // Cache nodes retain a copy of their inputs for comparison; opaque nodes do
    // not, so owned values can be moved without cloning.
    let current = if cache_enabled {
        Some(gathered.clone())
    } else {
        None
    };
    let should_run = {
        let node = registry.lock(id).ok_or(Error::UnknownNode(id))?;
        match &node.cache {
            CacheState::None => true,
            CacheState::Last { inputs } => {
                !node.has_run
                    || !inputs_equal(signature, inputs, current.as_ref().expect("cache node"))
            }
            CacheState::All { entries } => !entries.iter().any(|(cached, _)| {
                inputs_equal(signature, cached, current.as_ref().expect("cache node"))
            }),
        }
    };
    if !should_run {
        return Ok(());
    }

    let mut stage = {
        let mut node = registry.lock(id).ok_or(Error::UnknownNode(id))?;
        node.stage.take()
    }
    .ok_or(Error::UnknownNode(id))?;

    let mut io = Io::new(signature);
    for (index, value) in gathered.into_iter().enumerate() {
        if let Some(value) = value {
            io.set_input(index, value);
        }
    }

    let result = stage.run(&mut io).await;
    let outputs = io.into_outputs();

    {
        let mut node = registry.lock(id).ok_or(Error::UnknownNode(id))?;
        node.stage = Some(stage);
        if result.is_ok() {
            match (&mut node.cache, current) {
                (CacheState::None, _) => {}
                (CacheState::Last { inputs }, Some(current)) => *inputs = current,
                (CacheState::All { entries }, Some(current)) => {
                    entries.push((current, outputs.clone()));
                }
                _ => unreachable!("cache kind is fixed per node"),
            }
            node.outputs = outputs;
            node.has_run = true;
        }
    }

    result.map_err(|source| Error::Node {
        node: id,
        stage: signature.stage,
        source,
    })
}

fn inputs_equal(signature: &'static Signature, a: &[Option<Value>], b: &[Option<Value>]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (index, port) in signature.inputs.iter().enumerate() {
        let left = a.get(index).and_then(Option::as_ref);
        let right = b.get(index).and_then(Option::as_ref);
        match (left, right) {
            (None, None) => {}
            (Some(left), Some(right)) => match port.ops.eq_values(left, right) {
                Some(true) => {}
                _ => return false,
            },
            _ => return false,
        }
    }
    true
}

/// Drive a future to completion on the current thread.
///
/// The engine's futures only yield at user-defined `await` points and never
/// register a waker, so a busy-poll driver is sufficient.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    loop {
        match Pin::as_mut(&mut future).poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn noop_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
    const RAW: RawWaker = RawWaker::new(std::ptr::null(), &VTABLE);
    // SAFETY: the vtable's clone/wake/drop functions are all no-ops and never
    // dereference the null pointer.
    unsafe { Waker::from_raw(RAW) }
}
