//! Defines the graph structure that controls execution flow. The graph built
//! will be based on the nodes within a [`Registry`]. Multiple graphs can be
//! built from a single registry.
//!
//! It is technically possible to use multiple registries with a single graph,
//! but this would require that the registries have identical node Ids with
//! identical input/output types.
// TODO: The above could be made safe and easy to do, and likely is worth it
use daggy::{Dag, NodeIndex, Walker};
use std::collections::HashMap;

use crate::{
    NodeOutput,
    registry::Registry,
    stage::{EvalStrategy, ReevaluationRule},
    types::DataLabel,
};

// TODO: Still not perfect, need a nice way to make nodes connections even if they don't have any particular i/o relationship
/// Syntax sugar for building a graph
#[macro_export]
macro_rules! graph {
    // Handle explicitly named inputs and outputs
    (nodes: $nodes:expr, connections: { $($left_node:expr => $output:expr => $input:expr => $right_node:expr,)* }) => {
        {
            let mut graph = directed::Graph::from_node_indices($nodes);
            loop {
                $(
                    if let Err(e) = graph.connect($left_node, $right_node, directed::DataLabel::new($output), directed::DataLabel::new($input))
                    {
                        break Err(e);
                    }
                )*
                break Ok(graph);
            }
        }
    }
}

/// Directed Acryllic Graph representing the flow of execution in that pipeline.
/// Only operates on index and edge information - doesn't store actual state.
///
/// See [`Registry`] for where state comes in.
#[derive(Debug, Clone)]
pub struct Graph {
    pub(super) dag: Dag<usize, EdgeInfo>,
    pub(super) node_indices: HashMap<usize, NodeIndex>,
}

/// Information about connections between nodes, purely an implementation
/// detail of the graph.
#[derive(Debug, Clone)]
pub struct EdgeInfo {
    pub(super) source_output: DataLabel,
    pub(super) target_input: DataLabel,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            dag: Dag::new(),
            node_indices: HashMap::new(),
        }
    }

    /// Takes a slice of node indicies and adds them to an unconnected graph.
    /// These are the indices returned by [`Registry::register`]
    pub fn from_node_indices(node_indices: &[usize]) -> Self {
        let mut graph = Self::new();
        for i in node_indices {
            graph.add_node(*i);
        }
        graph
    }

    /// Adds a new node to the graph, by its [`Registry`] index.
    pub fn add_node(&mut self, id: usize) -> NodeIndex {
        let idx = self.dag.add_node(id);
        self.node_indices.insert(id, idx);
        idx
    }

    /// Connects the output of a node to the input of another node, resulting
    /// in a new graph edge. See [`Registry`]
    pub fn connect(
        &mut self,
        from_id: usize,
        to_id: usize,
        source_output: DataLabel,
        target_input: DataLabel,
    ) -> anyhow::Result<()> {
        let from_idx = self
            .node_indices
            .get(&from_id)
            .ok_or_else(|| anyhow::anyhow!("Source node {} not found", from_id))?;
        let to_idx = self
            .node_indices
            .get(&to_id)
            .ok_or_else(|| anyhow::anyhow!("Target node {} not found", to_id))?;
        self.dag
            .add_edge(
                *from_idx,
                *to_idx,
                EdgeInfo {
                    source_output,
                    target_input,
                },
            )
            .map_err(|e| anyhow::anyhow!("Failed to add edge: {:?}", e))?;

        Ok(())
    }

    /// Execute the graph in its current state, performing the entire flow of
    /// operations. This will find any non-lazy nodes and execute each of them,
    /// recursively executing all dependencies first in order to satisfy their
    /// input requirements.
    // TODO: Return a NodeOutput with all the results.
    // TODO: This may call things redundantly that don't have to be (eg, if an urgent node is a dep for another urgent node)
    pub fn execute(&self, registry: &mut Registry) -> anyhow::Result<()> {
        // Find all urgent nodes
        let mut urgent_nodes = Vec::new();

        for (_, idx) in &self.node_indices {
            let node_id = *self
                .dag
                .node_weight(*idx)
                .ok_or_else(|| anyhow::anyhow!("Node not found in graph"))?;

            let node = registry.get(node_id)?;
            if node.eval_strategy() == EvalStrategy::Urgent {
                urgent_nodes.push(idx);
            }
        }

        if urgent_nodes.is_empty() {
            return Err(anyhow::anyhow!("No urgent nodes found in graph"));
        }

        // Execute all urgent nodes (which will recursively execute dependencies)
        for node_idx in urgent_nodes {
            self.execute_node(*node_idx, registry)
                .map_err(|e| anyhow::anyhow!(e))?;
        }

        Ok(())
    }

    /// Execute a single node's stage within the graph. This will recursively execute
    /// all dependant parent nodes.
    fn execute_node(&self, idx: NodeIndex, registry: &mut Registry) -> anyhow::Result<()> {
        // Get the node ID
        let &node_id = self
            .dag
            .node_weight(idx)
            .ok_or_else(|| anyhow::anyhow!("Node not found in graph"))?;

        // First execute all parent nodes
        for parent in self.dag.parents(idx).iter(&self.dag) {
            let parent_idx = parent.1;
            self.execute_node(parent_idx, registry)?;
        }

        for parent in self.dag.parents(idx).iter(&self.dag) {
            let parent_idx = parent.1;
            let edge_idx = parent.0;

            let edge_info = self
                .dag
                .edge_weight(edge_idx)
                .ok_or_else(|| anyhow::anyhow!("Edge data not found"))?;

            let &parent_id = self
                .dag
                .node_weight(parent_idx)
                .ok_or_else(|| anyhow::anyhow!("Parent node not found in graph"))?;

            let (node, parent_node) = registry.get2_mut(node_id, parent_id)?;

            node.flow_data(
                parent_node,
                edge_info.source_output.clone(),
                edge_info.target_input.clone(),
            )?;
        }

        // Determine if we need to evaluate
        let node = registry.get_mut(node_id)?;
        if node.reeval_rule() == ReevaluationRule::Move || node.input_changed() {
            // TODO: Do something with the previous outputs, which are returned here
            // TODO: Handle ReevaluationRule::CacheAll
            node.eval()?;
            node.set_input_changed(false);
        }
        // TODO: ?

        Ok(())
    }

    /// This will take an output from the graph, either cloning it or removing it entirely, based on cache settings.
    pub fn get_output(&self, _registry: &mut Registry) -> NodeOutput {
        // TODO: Iterate through registry. For each node, see if it has an unconnected output. That is graph output!
        // Do not touch connected outputs
        // Later we will do something for inputs too
        todo!()
    }
}
