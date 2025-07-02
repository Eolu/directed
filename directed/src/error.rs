//! Errors and the graph trace system
use facet::Field;

use crate::{AnyNode, Graph, Registry, registry::NodeReflection};
use std::fmt::{self, Display, Formatter, Write};

/// Wrapper error type, wraps errors from this crate and stores a graph information with them.
#[derive(thiserror::Error, Debug)]
pub struct ErrorWithTrace<T: std::error::Error> {
    #[source]
    pub error: T,
    pub graph_trace: Option<GraphTrace>,
}

#[derive(thiserror::Error, Debug)]
pub enum InjectionError {
    #[error("Output '{0:?}' not found")]
    OutputNotFound(Option<&'static Field>),
    #[error("Output '{0:?}' type mismatch")]
    OutputTypeMismatch(Option<&'static Field>),
    #[error("Input '{0:?}' not found")]
    InputNotFound(Option<&'static Field>),
    #[error("Input '{0:?}' type mismatch")]
    InputTypeMismatch(Option<&'static Field>),
}

#[derive(thiserror::Error, Debug)]
pub enum NodeExecutionError {
    #[error(transparent)]
    NodesNotFoundInRegistry(#[from] NodesNotFoundError),
    #[error(transparent)]
    NodeNotFoundInGraph(#[from] NodeNotFoundInGraphError),
    #[error(transparent)]
    EdgeNotFoundInGraph(#[from] EdgeNotFoundInGraphError),
    #[error(transparent)]
    InputInjection(#[from] InjectionError),
    #[cfg(feature = "tokio")]
    #[error(transparent)]
    JoinError(#[from] tokio::task::JoinError),
}

#[derive(thiserror::Error, Debug)]
pub enum RegistryError {
    #[error(transparent)]
    NodesNotFoundInRegistry(#[from] NodesNotFoundError),
    #[error(transparent)]
    NodeTypeMismatch(#[from] NodeTypeMismatchError),
}

#[derive(thiserror::Error, Debug)]
pub enum EdgeCreationError {
    #[error(transparent)]
    NodesNotFound(#[from] NodesNotFoundError),
    #[error(transparent)]
    CycleError(daggy::WouldCycle<crate::EdgeInfo>),
}

#[derive(thiserror::Error, Debug)]
#[error("Invalid node type: (id:{got:?}). Expected: (id:{expected:?})")]
pub struct NodeTypeMismatchError {
    pub got: std::any::TypeId,
    pub expected: std::any::TypeId,
}

#[derive(thiserror::Error, Debug)]
#[error("Nodes with id `{0:?}` not found in registry")]
pub struct NodesNotFoundError(Vec<NodeReflection>);

impl From<&[NodeReflection]> for NodesNotFoundError {
    fn from(value: &[NodeReflection]) -> Self {
        Self(Vec::from(value))
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Node with index `{0:?}` not found in graph")]
pub struct NodeNotFoundInGraphError(daggy::NodeIndex);

impl From<daggy::NodeIndex> for NodeNotFoundInGraphError {
    fn from(value: daggy::NodeIndex) -> Self {
        Self(value)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Edge with index `{0:?}` not found in graph")]
pub struct EdgeNotFoundInGraphError(daggy::EdgeIndex);

impl From<daggy::EdgeIndex> for EdgeNotFoundInGraphError {
    fn from(value: daggy::EdgeIndex) -> Self {
        Self(value)
    }
}

impl<T: std::error::Error> Display for ErrorWithTrace<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.error)?;
        if let Some(graph_trace) = &self.graph_trace {
            writeln!(f, "{}", graph_trace.create_mermaid_graph())?;
        }
        Ok(())
    }
}

impl<T: std::error::Error> From<T> for ErrorWithTrace<T> {
    fn from(error: T) -> Self {
        Self {
            error,
            graph_trace: None,
        }
    }
}

impl<T: std::error::Error> ErrorWithTrace<T> {
    pub fn with_trace(self, trace: GraphTrace) -> Self {
        Self {
            error: self.error,
            graph_trace: Some(trace),
        }
    }
}

/// A trace of a graph, containing information about nodes and connections.
#[derive(Clone)]
pub struct GraphTrace {
    /// Information about each node in the graph.
    pub nodes: Vec<NodeInfo>,
    /// Information about each connection in the graph.
    pub connections: Vec<ConnectionInfo>,
}

impl std::fmt::Debug for GraphTrace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.create_mermaid_graph())
    }
}

/// Information about a node in the graph.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// The unique ID of the node.
    pub id: NodeReflection,
    /// The name of the node.
    pub name: &'static str,
    /// The input fields of the node.
    pub inputs: &'static [Field],
    /// The output fields of the node.
    pub outputs: &'static [Field],
    /// Used for debugging purposes
    pub highlighted: bool,
}

/// Information about a connection in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// The ID of the source node.
    pub source_id: NodeReflection,
    /// The output label of the source node.
    pub source_output: Option<&'static Field>,
    /// The ID of the target node.
    pub target_id: NodeReflection,
    /// The input label of the target node.
    pub target_input: Option<&'static Field>,
    /// Used for debugging purposes
    pub highlighted: bool,
}

// Extension to Registry to allow access to nodes by ID
impl Registry {
    /// Gets a node by its ID.
    pub fn get_node_by_id(&self, id: NodeReflection) -> Option<&Box<dyn AnyNode>> {
        self.0.get(id.id).map(|node| node.as_ref()).flatten()
    }
}

impl Graph {
    /// Generates a trace of the graph.
    pub fn generate_trace(&self, registry: &Registry) -> GraphTrace {
        let mut nodes = Vec::new();
        let mut connections = Vec::new();

        // Add node information
        for (&id, _) in &self.node_indices {
            if let Some(node) = registry.get_node_by_id(id) {
                let stage_shape = node.stage_shape();
                let node_info = NodeInfo {
                    id,
                    name: stage_shape.stage_name,
                    inputs: stage_shape.input_fields(),
                    outputs: stage_shape.output_fields(),
                    highlighted: false,
                };
                nodes.push(node_info);
            }
        }

        // Add connection information
        for edge in self.dag.raw_edges() {
            let source_idx = edge.source();
            let target_idx = edge.target();

            // Find the node IDs corresponding to the indices
            let source_id = self
                .node_indices
                .iter()
                .find(|(_, idx)| **idx == source_idx)
                .map(|(&id, _)| id);

            let target_id = self
                .node_indices
                .iter()
                .find(|(_, idx)| **idx == target_idx)
                .map(|(&id, _)| id);

            if let (Some(source_id), Some(target_id)) = (source_id, target_id) {
                let source_output = edge.weight.source_output;
                let target_input = edge.weight.target_input;
                let connection_info = ConnectionInfo {
                    source_id,
                    source_output,
                    target_id,
                    target_input,
                    highlighted: false,
                };
                connections.push(connection_info);
            }
        }

        GraphTrace { nodes, connections }
    }
}

impl GraphTrace {
    /// Emphasizes a node in the trace
    pub fn highlight_node(&mut self, node: NodeReflection) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node) {
            node.highlighted = true;
        }
    }

    /// Emphasizes a connection in the trace
    pub fn highlight_connection(
        &mut self,
        source_node: NodeReflection,
        source_output: Option<&'static Field>,
        target_node: NodeReflection,
        target_input: Option<&'static Field>,
    ) {
        if let Some(conn) = self.connections.iter_mut().find(|conn| {
            conn.source_id == source_node
                && conn.source_output == source_output
                && conn.target_id == target_node
                && conn.target_input == target_input
        }) {
            conn.highlighted = true;
        }
    }

    /// Creates a mermaid graph representing the graph.
    pub fn create_mermaid_graph(&self) -> String {
        const EMPHASIS_STYLE: &str = "stroke:yellow,stroke-width:3;";
        const SANITIZER: &str = " |-|.|:|/|\\";
        let mut result = String::new();

        // Note the unwraps in this function are fine. If they were to actually
        // panic there are deeper problems going on.

        // Start the Mermaid flowchart definition
        writeln!(&mut result, "```mermaid").unwrap();
        writeln!(&mut result, "flowchart TB").unwrap();

        // Create subgraphs for each node with its inputs and outputs
        for node in &self.nodes {
            // Create a subgraph for the node
            write!(&mut result, "    subgraph Node_{}_", node.id.id).unwrap();
            write!(&mut result, "[\"Node {} ({})\"]", node.id.id, node.name).unwrap();
            writeln!(&mut result, "").unwrap();

            // Define a node for each input port
            for input in node.inputs.iter() {
                let field_name = input.name;
                let ty = input.shape.to_string();
                writeln!(
                    &mut result,
                    "        {}_in_{}[/\"{}: {ty}\"\\]",
                    node.id.id,
                    field_name.replace(SANITIZER, "_"),
                    field_name
                )
                .unwrap();
            }

            // Define a node for each output port, unless this is a plain node.
            for output in node.outputs.iter() {
                let field_name = output.name;
                write!(
                    &mut result,
                    "        {}_out_{}[\\\"",
                    node.id.id,
                    field_name.replace(SANITIZER, "_")
                )
                .unwrap();
                write!(&mut result, "{}: ", field_name).unwrap();
                let type_name = &output.shape.to_string();
                write!(&mut result, "{type_name}").unwrap();
                writeln!(&mut result, "\"/]").unwrap();
            }

            writeln!(&mut result, "    end").unwrap();
            if node.highlighted {
                writeln!(
                    &mut result,
                    "    style Node_{}_ {EMPHASIS_STYLE}",
                    node.id.id
                )
                .unwrap();
            }
        }

        // Create the connections between nodes
        for (i, conn) in self.connections.iter().enumerate() {
            let source_name = conn.source_output.map(|n| n.name).unwrap_or("_");
            let target_name = conn.target_input.map(|n| n.name).unwrap_or("_");

            write!(
                &mut result,
                "    {}_out_{} ",
                conn.source_id.id,
                source_name.replace(SANITIZER, "_")
            )
            .unwrap();
            write!(&mut result, "--> ").unwrap();
            writeln!(
                &mut result,
                "{}_in_{}",
                conn.target_id.id,
                target_name.replace(SANITIZER, "_")
            )
            .unwrap();

            if conn.highlighted {
                writeln!(&mut result, "    linkStyle {i} {EMPHASIS_STYLE}").unwrap();
            }
        }

        // End the Mermaid diagram
        writeln!(&mut result, "```").unwrap();

        result
    }
}
