//! Errors and the graph trace system
use crate::{AnyNode, DataLabel, Graph, Registry};
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter, Write},
};

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
    OutputNotFound(DataLabel),
    #[error("Output '{0:?}' type mismatch")]
    OutputTypeMismatch(DataLabel),
    #[error("Input '{0:?}' not found")]
    InputNotFound(DataLabel),
    #[error("Input '{0:?}' type mismatch")]
    InputTypeMismatch(DataLabel),
    #[error("Input '{name}' type mismatch, expected '{expected}'")]
    InputTypeMismatchDetails {
        name: &'static str,
        expected: &'static str,
    },
    #[error("Unexpected references alive for `{0}`")]
    TooManyReferences(&'static str),
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
pub struct NodesNotFoundError(Vec<usize>);

impl From<&[usize]> for NodesNotFoundError {
    fn from(value: &[usize]) -> Self {
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
    pub id: usize,
    /// The name of the node.
    pub name: String,
    /// The input names of the node.
    pub inputs: Vec<DataLabel>,
    /// The output names of the node.
    pub outputs: Vec<DataLabel>,
    /// Used for debugging purposes
    pub highlighted: bool,
}

/// Information about a connection in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// The ID of the source node.
    pub source_id: usize,
    /// The output label of the source node.
    pub source_output: DataLabel,
    /// The ID of the target node.
    pub target_id: usize,
    /// The input label of the target node.
    pub target_input: DataLabel,
    /// Used for debugging purposes
    pub highlighted: bool,
}

// Extension to Registry to allow access to nodes by ID
impl Registry {
    /// Gets a node by its ID.
    pub fn get_node_by_id(&self, id: usize) -> Option<&Box<dyn AnyNode>> {
        self.0.get(id).map(|node| node.as_ref()).flatten()
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
                let node_info = NodeInfo {
                    id,
                    name: node.stage_name().to_string(),
                    inputs: node.input_names().collect(),
                    outputs: node.output_names().collect(),
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
                let source_output = edge.weight.source_output.clone();
                let target_input = edge.weight.target_input.clone();
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
    pub fn highlight_node(&mut self, node: usize) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.id == node) {
            node.highlighted = true;
        }
    }

    /// Emphasizes a connection in the trace
    pub fn highlight_connection(
        &mut self,
        source_node: usize,
        source_output: DataLabel,
        target_node: usize,
        target_input: DataLabel,
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
            write!(&mut result, "    subgraph Node_{}_", node.id).unwrap();
            write!(&mut result, "[\"Node {} ({})\"]", node.id, node.name).unwrap();
            writeln!(&mut result, "").unwrap();

            // Define a node for each input port
            for input in &node.inputs {
                let type_name = if let Some(type_name) = &input.type_name {
                    type_name
                } else {
                    "?"
                };
                writeln!(
                    &mut result,
                    "        {}_in_{}[/\"{}: {type_name}\"\\]",
                    node.id,
                    input
                        .name
                        .as_ref()
                        .map(|name| name.replace(SANITIZER, "_"))
                        .unwrap_or_else(|| "!".into()),
                    input.name.as_ref().unwrap_or_else(|| &Cow::Borrowed("!"))
                )
                .unwrap();
            }

            // Define a node for each output port, unless this is a plain node.
            let has_unnamed_output = node.outputs.len() == 1
                && "_"
                    == node.outputs[0]
                        .name
                        .as_ref()
                        .unwrap_or_else(|| &Cow::Borrowed("!"));
            if !(has_unnamed_output
                && Some(std::borrow::Cow::Borrowed("()")) == node.outputs[0].type_name)
            {
                for output in &node.outputs {
                    write!(
                        &mut result,
                        "        {}_out_{}[\\\"",
                        node.id,
                        output
                            .name
                            .as_ref()
                            .map(|name| name.replace(SANITIZER, "_"))
                            .unwrap_or_else(|| "!".into())
                    )
                    .unwrap();
                    if !has_unnamed_output {
                        write!(
                            &mut result,
                            "{}: ",
                            output.name.as_ref().unwrap_or_else(|| &Cow::Borrowed("!"))
                        )
                        .unwrap();
                    }
                    if let Some(type_name) = &output.type_name {
                        write!(&mut result, "{type_name}").unwrap();
                    }
                    writeln!(&mut result, "\"/]").unwrap();
                }
            }

            writeln!(&mut result, "    end").unwrap();
            if node.highlighted {
                writeln!(&mut result, "    style Node_{}_ {EMPHASIS_STYLE}", node.id).unwrap();
            }
        }

        // Create the connections between nodes
        for (i, conn) in self.connections.iter().enumerate() {
            write!(
                &mut result,
                "    {}_out_{} ",
                conn.source_id,
                conn.source_output
                    .name
                    .as_ref()
                    .map(|name| name.replace(SANITIZER, "_"))
                    .unwrap_or_else(|| "!".into())
            )
            .unwrap();
            write!(&mut result, "--> ").unwrap();
            writeln!(
                &mut result,
                "{}_in_{}",
                conn.target_id,
                conn.target_input
                    .name
                    .as_ref()
                    .map(|name| name.replace(SANITIZER, "_"))
                    .unwrap_or_else(|| "!".into())
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
