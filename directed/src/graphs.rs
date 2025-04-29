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
    registry::Registry, stage::{EvalStrategy, ReevaluationRule}, types::DataLabel, EdgeCreationError, EdgeNotFoundInGraphError, Error, NodeExecutionError, NodeNotFoundInGraphError, NodeOutput, NodesNotFoundError
};

// TODO: Still not perfect, need a nice way to make nodes connections even if they don't have any particular i/o relationship
/// Syntax sugar for building a graph
#[macro_export]
macro_rules! graph {
    // Handle explicitly named inputs and outputs
    (nodes: $nodes:expr, connections: { $($left_node:ident: $output:expr => $right_node:ident: $input:expr,)* }) => {
        {
            #[allow(unused_mut)]
            let mut graph = directed::Graph::from_node_indices($nodes.as_ref());
            loop {
                $(
                    if let Err(e) = graph.connect($left_node, $right_node, directed::DataLabel::new_const(stringify!($output)), directed::DataLabel::new_const(stringify!($input)))
                    {
                        break Err(e);
                    }
                )*
                break Ok(graph) as Result<directed::Graph, directed::EdgeCreationError>;
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
        // TODO: Use node weights in a better way
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
    ) -> Result<(), EdgeCreationError> {
        let from_idx = self
            .node_indices
            .get(&from_id)
            .ok_or_else(|| NodesNotFoundError::from(&[from_id] as &[usize]))?;
        let to_idx = self
            .node_indices
            .get(&to_id)
            .ok_or_else(|| NodesNotFoundError::from(&[to_id] as &[usize]))?;
        self.dag
            .add_edge(
                *from_idx,
                *to_idx,
                EdgeInfo {
                    source_output,
                    target_input,
                },
            )
            .map_err(|e| EdgeCreationError::CycleError(e))?;

        Ok(())
    }

    /// Execute the graph in its current state, performing the entire flow of
    /// operations. This will find any non-lazy nodes and execute each of them,
    /// recursively executing all dependencies first in order to satisfy their
    /// input requirements.
    // TODO: Return a NodeOutput with all the results.
    // TODO: This may call things redundantly that don't have to be (eg, if an urgent node is a dep for another urgent node)
    pub fn execute(&self, registry: &mut Registry) -> Result<(), Error<NodeExecutionError>> {
        // Find all urgent nodes
        let mut urgent_nodes = Vec::new();
        let base_trace = self.generate_trace(registry);

        for (_, idx) in &self.node_indices {
            let node_id = *self
                .dag
                .node_weight(*idx)
                .ok_or_else(|| Error::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(*idx))))
                .map_err(|err| err.with_trace(base_trace.clone()))?;

            let node = match registry.get(node_id) {
                Some(node) => node,
                None => {
                    return Err(Error::from(NodeExecutionError::from(NodesNotFoundError::from(&[node_id] as &[usize])))
                        .with_trace(base_trace));
                }
            };
            if node.eval_strategy() == EvalStrategy::Urgent {
                urgent_nodes.push(idx);
            }
        }

        // Execute all urgent nodes (which will recursively execute dependencies)
        for node_idx in urgent_nodes {
            self.execute_node(*node_idx, registry)?;
        }

        Ok(())
    }

    /// Execute a single node's stage within the graph. This will recursively execute
    /// all dependant parent nodes.
    fn execute_node(&self, idx: NodeIndex, registry: &mut Registry) -> Result<(), Error<NodeExecutionError>> {
        // TODO: Better if we don't have to do this here
        let top_trace = self.generate_trace(registry);
        
        // Get the node ID
        let &node_id = self
            .dag
            .node_weight(idx)
            .ok_or_else(|| Error::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(idx))))
            .map_err(|err| err.with_trace(top_trace.clone()))?;

        // First execute all parent nodes
        for parent in self.dag.parents(idx).iter(&self.dag) {
            let parent_idx = parent.1;
            self.execute_node(parent_idx, registry)?;
        }

        for parent in self.dag.parents(idx).iter(&self.dag) {
            let parent_idx = parent.1;
            let edge_idx = parent.0;

            let &parent_id = self
                .dag
                .node_weight(parent_idx)
                .ok_or_else(|| Error::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(parent_idx))))
                .map_err(|err| err.with_trace(top_trace.clone()))?;

            let edge_info = self
                .dag
                .edge_weight(edge_idx)
                .ok_or_else(|| Error::from(NodeExecutionError::from(EdgeNotFoundInGraphError::from(edge_idx))))
                .map_err(|err| err.with_trace({
                    let mut trace = self.generate_trace(registry);
                    trace.highlight_node(parent_id);
                    trace.highlight_node(node_id);
                    trace
                }))?;

            let (node, parent_node) = registry.get2_mut(node_id, parent_id)
                .map_err(|err| Error::from(NodeExecutionError::from(err)))
                .map_err(|err| err.with_trace(top_trace.clone()))?;

            parent_node.flow_data(
                node,
                edge_info.source_output.clone(),
                edge_info.target_input.clone(),
            ).map_err(|err| Error::from(NodeExecutionError::from(err)))
             .map_err(|err| err.with_trace({
                let mut trace = self.generate_trace(registry);
                trace.highlight_node(parent_id);
                trace.highlight_node(node_id);
                trace.highlight_connection(parent_id, edge_info.source_output.clone(), node_id, edge_info.target_input.clone());
                trace
             }))?;
        }

        // Get error trace info before it's too late

        // Determine if we need to evaluate
        let node = match registry.get_mut(node_id) {
            Some(node) => node,
            None => {
                return Err(Error::from(NodeExecutionError::from(NodesNotFoundError::from(&[node_id] as &[usize])))
                    .with_trace(self.generate_trace(registry)));
            }
        };
        if node.reeval_rule() == ReevaluationRule::Move || node.input_changed() {
            // TODO: Do something with the previous outputs, which are returned here
            // TODO: Handle ReevaluationRule::CacheAll
            node.eval()
                .map_err(|err| Error::from(NodeExecutionError::from(err)))
                .map_err(|err| err.with_trace(top_trace))?;
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

    /// Execute the graph asynchronously
    #[cfg(feature = "tokio")]
    pub async fn execute_async(self: std::sync::Arc<Self>, registry: tokio::sync::Mutex<Registry>) -> Result<(), Error<NodeExecutionError>> {
        // Find all urgent nodes
        let mut urgent_nodes = Vec::new();
        let top_trace = self.generate_trace(&*registry.lock().await);

        for (_, idx) in &self.node_indices {
            let node_id = *self
                .dag
                .node_weight(*idx)
                .ok_or_else(|| Error::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(*idx))))
                .map_err(|err| err.with_trace(top_trace.clone()))?;

            let registry = registry.lock().await;
            let node = match registry.get(node_id) {
                Some(node) => node,
                None => {
                    return Err(Error::from(NodeExecutionError::from(NodesNotFoundError::from(&[node_id] as &[usize])))
                        .with_trace(top_trace.clone()));
                }
            };
            if node.eval_strategy() == EvalStrategy::Urgent {
                urgent_nodes.push(idx);
            }
        }

        // Guard the registry with a mutex
        let registry_ref = std::sync::Arc::new(registry);

        // Execute all urgent nodes (which will recursively execute dependencies)
        let mut futures = Vec::new();
        for node_idx in urgent_nodes {
            futures.push(self.clone().execute_node_async(*node_idx, top_trace.clone(), registry_ref.clone()));
        }

        // Wait for all tasks to complete
        for future in futures {
            future.await?;
        }

        Ok(())
    }

    /// Execute a single node's stage asynchronously within the graph. This will recursively execute
    /// all dependant parent nodes in parallel.
    #[cfg(feature = "tokio")]
    #[async_recursion::async_recursion]
    async fn execute_node_async(self: std::sync::Arc<Self> , idx: NodeIndex, top_trace: crate::GraphTrace, registry: std::sync::Arc<tokio::sync::Mutex<Registry>>) -> Result<(), Error<NodeExecutionError>> {
        // Get the node ID
        let &node_id = self
            .dag
            .node_weight(idx)
            .ok_or_else(|| Error::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(idx))))
            .map_err(|err| err.with_trace(top_trace.clone()))?;

        // Get all parent nodes
        let parents: Vec<_> = self.dag.parents(idx).iter(&self.dag).collect();
        
        // Execute all parent nodes in parallel
        if !parents.is_empty() {
            // Guard the registry with a mutex
            let mut parent_handles = tokio::task::JoinSet::new();
            for parent in &parents {
                let parent_idx = parent.1;
                parent_handles.spawn(self.clone().execute_node_async(parent_idx, top_trace.clone(), registry.clone()));
            }
            
            // Wait for all parent nodes to complete
            for res in parent_handles.join_all().await {
                res.map_err(|err| err.with_trace(top_trace.clone()))?;
            }
        }

        // Flow data from all parents to this node
        {
            // Registry lock for flowing data between nodes
            let mut registry = registry.lock().await;
            for parent in parents {
                let parent_idx = parent.1;
                let edge_idx = parent.0;

                let &parent_id = self
                    .dag
                    .node_weight(parent_idx)
                    .ok_or_else(|| Error::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(parent_idx))))
                    .map_err(|err| err.with_trace(top_trace.clone()))?;

                let edge_info = self
                    .dag
                    .edge_weight(edge_idx)
                    .ok_or_else(|| Error::from(NodeExecutionError::from(EdgeNotFoundInGraphError::from(edge_idx))))
                    .map_err(|err| err.with_trace({
                        let mut trace = top_trace.clone();
                        trace.highlight_node(parent_id);
                        trace.highlight_node(node_id);
                        trace
                    }))?;

                let (node, parent_node) = registry.get2_mut(node_id, parent_id)
                .map_err(|err| Error::from(NodeExecutionError::from(err)))
                .map_err(|err| err.with_trace(top_trace.clone()))?;

                parent_node.flow_data(
                    node,
                    edge_info.source_output.clone(),
                    edge_info.target_input.clone(),
                ).map_err(|err| Error::from(NodeExecutionError::from(err)))
                .map_err(|err| err.with_trace({
                    let mut trace = top_trace.clone();
                    trace.highlight_node(parent_id);
                    trace.highlight_node(node_id);
                    trace.highlight_connection(parent_id, edge_info.source_output.clone(), node_id, edge_info.target_input.clone());
                    trace
                }))?;
            }
        }

        // Pull the node out of the registry
        let mut node = {
            let mut registry = registry.lock().await;
            // Determine if we need to evaluate
            match registry.take_node(node_id) {
                Some(node) => node,
                None => {
                    let node_ids: &[usize] = &[node_id];
                    return Err(Error::from(NodeExecutionError::from(NodesNotFoundError::from(node_ids)))
                        .with_trace(top_trace));
                }
            }
        };

        // Determine if we need to evaluate
        if node.reeval_rule() == ReevaluationRule::Move || node.input_changed() {
            // Evaluate asynchronously
            // TODO: Do someting with output
            let _ = node.eval_async().await
                .map_err(|_| Error::from(NodeExecutionError::from(crate::InjectionError::TooManyReferences("async task failed"))))?;
                
            node.set_input_changed(false);
        }

        println!("REPLACE NODE {node_id}");
        // Eval is done, reinsert node
        registry.lock().await.replace_node(node_id, node);

        Ok(())
    }
}
