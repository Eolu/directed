//! Graph connectivity: a stateless description of which node outputs feed
//! which node inputs. Execution lives in [`crate::execute`].

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use crate::registry::NodeId;
use crate::signature::{InputPort, OutputPort};
use crate::stage::StageHandle;

/// A typed reference to an output port of a registered node.
pub struct PortOut<T> {
    pub(crate) node: NodeId,
    pub(crate) index: u16,
    pub(crate) port: &'static OutputPort,
    pub(crate) _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for PortOut<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for PortOut<T> {}

impl<T> PortOut<T> {
    /// Intended for use by `#[stage]`-generated handle methods.
    #[doc(hidden)]
    pub fn new(node: NodeId, index: u16, port: &'static OutputPort) -> Self {
        Self {
            node,
            index,
            port,
            _marker: PhantomData,
        }
    }
}

/// A typed reference to an input port of a registered node.
pub struct PortIn<T> {
    pub(crate) node: NodeId,
    pub(crate) index: u16,
    pub(crate) port: &'static InputPort,
    pub(crate) _marker: PhantomData<fn(T)>,
}

impl<T> Clone for PortIn<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for PortIn<T> {}

impl<T> PortIn<T> {
    /// Intended for use by `#[stage]`-generated handle methods.
    #[doc(hidden)]
    pub fn new(node: NodeId, index: u16, port: &'static InputPort) -> Self {
        Self {
            node,
            index,
            port,
            _marker: PhantomData,
        }
    }
}

/// Errors produced while wiring a graph.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("edge {from} -> {to} would create a cycle")]
    Cycle { from: NodeId, to: NodeId },

    #[error("node {0} is not registered")]
    UnknownNode(NodeId),

    #[error("node {node} has no port named `{port}`")]
    UnknownPort { node: NodeId, port: String },

    #[error(
        "cannot connect output `{from_port}` (`{from_type}`) to input `{to_port}` (`{to_type}`)"
    )]
    TypeMismatch {
        from_port: &'static str,
        from_type: &'static str,
        to_port: &'static str,
        to_type: &'static str,
    },
}

/// A connection between an output port of one node and an input port of another.
#[derive(Clone, Copy)]
pub struct Edge {
    pub from: NodeId,
    pub from_index: u16,
    pub from_port: &'static OutputPort,
    pub to: NodeId,
    pub to_index: u16,
    pub to_port: &'static InputPort,
}

/// A stateless graph over node ids. See [`Registry`](crate::Registry) for state.
#[derive(Default, Clone)]
pub struct Graph {
    nodes: Vec<NodeId>,
    node_set: HashSet<NodeId>,
    edges: Vec<Edge>,
    outgoing: HashMap<NodeId, Vec<usize>>,
    incoming: HashMap<NodeId, Vec<usize>>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// The node ids that are part of this graph, in insertion order.
    pub fn nodes(&self) -> &[NodeId] {
        &self.nodes
    }

    /// All edges in the graph.
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    pub(crate) fn incoming_edges(&self, id: NodeId) -> &[usize] {
        self.incoming.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn ensure_node(&mut self, id: NodeId) {
        if self.node_set.insert(id) {
            self.nodes.push(id);
        }
    }

    fn would_cycle(&self, from: NodeId, to: NodeId) -> bool {
        if from == to {
            return true;
        }
        let mut stack = vec![to];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if node == from {
                return true;
            }
            if !seen.insert(node) {
                continue;
            }
            if let Some(edge_indices) = self.outgoing.get(&node) {
                for &edge_index in edge_indices {
                    stack.push(self.edges[edge_index].to);
                }
            }
        }
        false
    }

    fn add_edge_checked(&mut self, edge: Edge) -> Result<(), BuildError> {
        if self.would_cycle(edge.from, edge.to) {
            return Err(BuildError::Cycle {
                from: edge.from,
                to: edge.to,
            });
        }
        let edge_index = self.edges.len();
        self.edges.push(edge);
        self.outgoing.entry(edge.from).or_default().push(edge_index);
        self.incoming.entry(edge.to).or_default().push(edge_index);
        Ok(())
    }

    /// Remove all edges and nodes from the graph.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Accumulates nodes and connections before producing a [`Graph`].
#[derive(Default)]
pub struct GraphBuilder {
    graph: Graph,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node (idempotent).
    pub fn add<H: StageHandle>(&mut self, handle: &H) -> NodeId {
        let id = handle.id();
        self.graph.ensure_node(id);
        id
    }

    /// Connect a typed output to a typed input. The compiler enforces that the
    /// two ports carry the same value type.
    pub fn connect<T>(&mut self, from: PortOut<T>, to: PortIn<T>) -> Result<(), BuildError> {
        if from.port.ops.type_id != to.port.ops.type_id {
            return Err(BuildError::TypeMismatch {
                from_port: from.port.name,
                from_type: from.port.ops.type_name,
                to_port: to.port.name,
                to_type: to.port.ops.type_name,
            });
        }
        self.graph.ensure_node(from.node);
        self.graph.ensure_node(to.node);
        self.graph.add_edge_checked(Edge {
            from: from.node,
            from_index: from.index,
            from_port: from.port,
            to: to.node,
            to_index: to.index,
            to_port: to.port,
        })
    }

    /// Connect by name, validating against the registry's static signatures.
    /// Useful for runtime rewiring where typed port handles are not available.
    pub fn connect_by_name(
        &mut self,
        from: NodeId,
        from_output: &str,
        to: NodeId,
        to_input: &str,
        registry: &crate::Registry,
    ) -> Result<(), BuildError> {
        let (from_index, from_port) = {
            let node = registry.lock(from).ok_or(BuildError::UnknownNode(from))?;
            let outputs = &node.signature().outputs;
            let index = outputs
                .iter()
                .position(|p| p.name == from_output)
                .ok_or_else(|| BuildError::UnknownPort {
                    node: from,
                    port: from_output.to_string(),
                })?;
            (index as u16, &outputs[index])
        };
        let (to_index, to_port) = {
            let node = registry.lock(to).ok_or(BuildError::UnknownNode(to))?;
            let inputs = &node.signature().inputs;
            let index = inputs
                .iter()
                .position(|p| p.name == to_input)
                .ok_or_else(|| BuildError::UnknownPort {
                    node: to,
                    port: to_input.to_string(),
                })?;
            (index as u16, &inputs[index])
        };
        if from_port.ops.type_id != to_port.ops.type_id {
            return Err(BuildError::TypeMismatch {
                from_port: from_port.name,
                from_type: from_port.ops.type_name,
                to_port: to_port.name,
                to_type: to_port.ops.type_name,
            });
        }
        self.graph.ensure_node(from);
        self.graph.ensure_node(to);
        self.graph.add_edge_checked(Edge {
            from,
            from_index,
            from_port,
            to,
            to_index,
            to_port,
        })
    }

    pub fn build(self) -> Graph {
        self.graph
    }
}

/// Build a [`Graph`] from typed connections.
///
/// ```ignore
/// let graph = graph! {
///     nodes: [node_1, node_2, node_3],
///     connections: {
///         node_1: out => node_2: input,
///         node_2: out => node_3: input,
///     }
/// }?;
/// ```
#[macro_export]
macro_rules! graph {
    (
        nodes: [$($node:ident),* $(,)?],
        connections: { $($left:ident : $out:ident => $right:ident : $in_:ident),* $(,)? }
    ) => {{
        (|| -> ::std::result::Result<$crate::Graph, $crate::BuildError> {
            let mut builder = $crate::GraphBuilder::new();
            $( let _ = builder.add(&$node); )*
            $( builder.connect($left.$out(), $right.$in_())?; )*
            Ok(builder.build())
        })()
    }};
}
