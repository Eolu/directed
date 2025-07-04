//! Defines the graph structure that controls execution flow. The graph built
//! will be based on the nodes within a [`Registry`]. Multiple graphs can be
//! built from a single registry.
use daggy::{Dag, EdgeIndex, NodeIndex, Walker};
use std::collections::HashMap;

use crate::{
    EdgeCreationError, EdgeNotFoundInGraphError, ErrorWithTrace, GraphTrace, NodeExecutionError,
    NodeId, NodeNotFoundInGraphError, NodesNotFoundError, Stage,
    registry::{NodeReflection, Registry},
    stage::{EvalStrategy, ReevaluationRule},
};

#[macro_export]
macro_rules! graph_internal {
    // Connect named output to named input
    ($graph:expr => $left_node:ident: $output:ident => $right_node:ident: $input:ident,) => {
        $graph.connect(
            $left_node,
            $right_node,
            Some(
                $left_node
                    .stage_shape()
                    .outputs
                    .iter()
                    .find(|field| field.name == stringify!($output))
                    .expect("Output not found in stage"),
            ),
            Some(
                $right_node
                    .stage_shape()
                    .inputs
                    .iter()
                    .find(|field| field.name == stringify!($input))
                    .expect("Input not found in stage"),
            ),
        )
    };
    // Connect unnamed output to named input
    ($graph:expr => $left_node:ident => $right_node:ident: $input:ident,) => {
        $graph.connect(
            $left_node,
            $right_node,
            None,
            Some(
                $right_node
                    .stage_shape()
                    .inputs
                    .iter()
                    .find(|field| field.name == stringify!($input))
                    .expect("Input not found in stage"),
            ),
        )
    };
    // Connect nodes but do not associate any inputs or outputs
    ($graph:expr => $left_node:ident => $right_node:ident,) => {
        $graph.connect($left_node, $right_node, None, None)
    };
}

/// Syntax sugar for building a graph
#[macro_export]
macro_rules! graph {
    // Handle explicitly named inputs and outputs
    (nodes: ($($nodes:expr),*), connections: { $($left_node:ident$(: $output:ident)? => $right_node:ident$(: $input:ident)?,)* }) => {
        {
            #[allow(unused_mut)]
            let mut graph = directed::Graph::from_node_ids(&[$($nodes.into()),*]);
            loop {
                $(
                    if let Err(e) = graph_internal!(graph => $left_node $(: $output)? => $right_node $(: $input)?,)
                    {
                        break Err(e);
                    }
                )*
                break Ok(graph) as Result<directed::Graph, directed::EdgeCreationError>;
            }
        }
    }
}

/// Used to reflect on types, important for node connections
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct TypeReflection {
    pub name: &'static str,
    pub ty: &'static str,
}

impl std::fmt::Display for TypeReflection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.ty)
    }
}

/// Directed Acryllic Graph representing the flow of execution in that pipeline.
/// Only operates on index and edge information - doesn't store actual state.
///
/// See [`Registry`] for where state comes in.
#[derive(Debug, Clone)]
pub struct Graph {
    pub(super) dag: Dag<NodeReflection, EdgeInfo>,
    pub(super) node_indices: HashMap<NodeReflection, NodeIndex>,
}

/// Information about connections between nodes, purely an implementation
/// detail of the graph.
#[derive(Debug, Clone)]
pub struct EdgeInfo {
    pub(super) source_output: Option<&'static TypeReflection>,
    pub(super) target_input: Option<&'static TypeReflection>,
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
    pub fn from_node_ids(node_ids: &[NodeReflection]) -> Self {
        let mut graph = Self::new();
        for i in node_ids {
            graph.add_node(*i);
        }
        graph
    }

    /// Adds a new node to the graph, by its [`Registry`] index.
    pub fn add_node(&mut self, id: impl Into<NodeReflection>) -> NodeIndex {
        let id = id.into();
        let idx = self.dag.add_node(id);
        self.node_indices.insert(id, idx);
        idx
    }

    /// Connects the output of a node to the input of another node, resulting
    /// in a new graph edge. See [`Registry`]
    pub fn connect<S0: Stage, S1: Stage>(
        &mut self,
        from_id: NodeId<S0>,
        to_id: NodeId<S1>,
        source_output: Option<&'static TypeReflection>,
        target_input: Option<&'static TypeReflection>,
    ) -> Result<(), EdgeCreationError> {
        let from_idx = self
            .node_indices
            .get(&from_id.clone().into())
            .ok_or_else(|| {
                NodesNotFoundError::from(
                    &[from_id.into()] as &[NodeReflection; 1] as &[NodeReflection]
                )
            })?;
        let to_idx = self
            .node_indices
            .get(&to_id.clone().into())
            .ok_or_else(|| {
                NodesNotFoundError::from(
                    &[to_id.into()] as &[NodeReflection; 1] as &[NodeReflection]
                )
            })?;
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
    pub fn execute(
        &self,
        registry: &mut Registry,
    ) -> Result<(), ErrorWithTrace<NodeExecutionError>> {
        let top_trace = self.generate_trace(registry);
        // Execute all urgent nodes (which will recursively execute dependencies)
        for node_idx in self
            .urgent_nodes(registry)
            .map_err(ErrorWithTrace::from)
            .map_err(|err| err.with_trace(top_trace.clone()))?
        {
            self.execute_node(*node_idx, top_trace.clone(), registry)?;
        }

        Ok(())
    }

    /// Execute the graph asynchronously
    #[cfg(feature = "tokio")]
    pub async fn execute_async(
        self: std::sync::Arc<Self>,
        registry: tokio::sync::Mutex<Registry>,
    ) -> Result<(), ErrorWithTrace<NodeExecutionError>> {
        let top_trace = self.generate_trace(&*registry.lock().await);

        let urgent_nodes = self
            .urgent_nodes(&*registry.lock().await)
            .map_err(ErrorWithTrace::from)
            .map_err(|err| err.with_trace(top_trace.clone()))?;

        // Guard the registry with a mutex
        let registry_ref = std::sync::Arc::new(registry);

        // Execute all urgent nodes (which will recursively execute dependencies)
        let mut futures = Vec::new();
        for node_idx in urgent_nodes {
            futures.push(self.clone().execute_node_async(
                *node_idx,
                top_trace.clone(),
                registry_ref.clone(),
            ));
        }

        // Wait for all tasks to complete
        for future in futures {
            future.await?;
        }

        Ok(())
    }

    /// Execute a single node's stage within the graph. This will recursively execute
    /// all dependant parent nodes.
    fn execute_node(
        &self,
        idx: NodeIndex,
        top_trace: GraphTrace,
        registry: &mut Registry,
    ) -> Result<(), ErrorWithTrace<NodeExecutionError>> {
        // Get the node ID
        let node_id = self
            .get_node_id_from_node_index(idx)
            .map_err(|err| ErrorWithTrace::from(NodeExecutionError::from(err)))
            .map_err(|err| err.with_trace(top_trace.clone()))?;

        // Get all parent nodes
        let parents: Vec<_> = self.dag.parents(idx).iter(&self.dag).collect();

        // First execute all parent nodes
        for parent in parents.iter() {
            let parent_idx = parent.1;
            self.execute_node(parent_idx, top_trace.clone(), registry)?;
        }

        // Flow data from all parents to this node
        self.flow_data(registry, top_trace.clone(), node_id, &parents)?;

        // Get mutable ref to node
        let node = registry.get_node_any_mut(node_id).ok_or_else(|| {
            ErrorWithTrace::from(NodeExecutionError::from(NodesNotFoundError::from(
                &[node_id.into()] as &[NodeReflection],
            )))
            .with_trace(top_trace.clone())
        })?;

        // Determine if we need to evaluate
        if node.reeval_rule() == ReevaluationRule::Move || node.input_changed() {
            // TODO: Do something with the previous outputs, which are returned here
            if node.reeval_rule() == ReevaluationRule::CacheAll {
                // TODO: Does this make sense? Does the macro handle enough to do nothing special here?
                node.eval()
                    .map_err(|err| ErrorWithTrace::from(NodeExecutionError::from(err)))
                    .map_err(|err| err.with_trace(top_trace))?;
            } else {
                node.eval()
                    .map_err(|err| ErrorWithTrace::from(NodeExecutionError::from(err)))
                    .map_err(|err| err.with_trace(top_trace))?;
            }
            node.set_input_changed(false);
        }

        Ok(())
    }

    /// Execute a single node's stage asynchronously within the graph. This will recursively execute
    /// all dependant parent nodes in parallel.
    #[cfg(feature = "tokio")]
    #[async_recursion::async_recursion]
    async fn execute_node_async(
        self: std::sync::Arc<Self>,
        idx: NodeIndex,
        top_trace: GraphTrace,
        registry: std::sync::Arc<tokio::sync::Mutex<Registry>>,
    ) -> Result<(), ErrorWithTrace<NodeExecutionError>> {
        // Get the node ID
        let node_id = self
            .get_node_id_from_node_index(idx)
            .map_err(|err| ErrorWithTrace::from(NodeExecutionError::from(err)))
            .map_err(|err| err.with_trace(top_trace.clone()))?;

        // Get all parent nodes
        let parents: Vec<_> = self.dag.parents(idx).iter(&self.dag).collect();

        // Execute all parent nodes in parallel
        if !parents.is_empty() {
            // Guard the registry with a mutex
            let mut parent_handles = tokio::task::JoinSet::new();
            for parent in &parents {
                let parent_idx = parent.1;
                parent_handles.spawn(self.clone().execute_node_async(
                    parent_idx,
                    top_trace.clone(),
                    registry.clone(),
                ));
            }

            // Wait for all parent nodes to complete
            for res in parent_handles.join_all().await {
                res.map_err(|err| err.with_trace(top_trace.clone()))?;
            }
        }

        // Flow data from all parents to this node
        self.flow_data(
            &mut *registry.lock().await,
            top_trace.clone(),
            node_id,
            &parents,
        )?;

        // Pull the node out of the registry
        let mut node = {
            let mut registry = registry.lock().await;
            // Determine if we need to evaluate
            registry.take_node(node_id).await.ok_or_else(|| {
                ErrorWithTrace::from(NodeExecutionError::from(NodesNotFoundError::from(
                    &[node_id.into()] as &[NodeReflection],
                )))
                .with_trace(top_trace)
            })?
        };

        // Determine if we need to evaluate
        if node.reeval_rule() == ReevaluationRule::Move || node.input_changed() {
            // Evaluate asynchronously
            // TODO: Do someting with output
            let _ = node.eval_async().await.map_err(|err| {
                ErrorWithTrace::from(NodeExecutionError::from(err))
            })?;

            node.set_input_changed(false);
        }

        // Eval is done, reinsert node
        registry.lock().await.replace_node(node_id, node);

        Ok(())
    }

    /// Flow outputs from all parent nodes to a node's inputs
    fn flow_data(
        &self,
        registry: &mut Registry,
        top_trace: GraphTrace,
        node_id: NodeReflection,
        parents: &[(EdgeIndex, NodeIndex)],
    ) -> Result<(), ErrorWithTrace<NodeExecutionError>> {
        for parent in parents {
            let parent_idx = parent.1;
            let edge_idx = parent.0;

            let &parent_id = self
                .dag
                .node_weight(parent_idx)
                .ok_or_else(|| {
                    ErrorWithTrace::from(NodeExecutionError::from(NodeNotFoundInGraphError::from(
                        parent_idx,
                    )))
                })
                .map_err(|err| err.with_trace(top_trace.clone()))?;

            let edge_info = self
                .dag
                .edge_weight(edge_idx)
                .ok_or_else(|| {
                    ErrorWithTrace::from(NodeExecutionError::from(EdgeNotFoundInGraphError::from(
                        edge_idx,
                    )))
                })
                .map_err(|err| {
                    err.with_trace({
                        let mut trace = top_trace.clone();
                        trace.highlight_node(parent_id);
                        trace.highlight_node(node_id);
                        trace
                    })
                })?;

            let (node, parent_node) = registry
                .get2_nodes_any_mut(node_id, parent_id)
                .map_err(|err| ErrorWithTrace::from(NodeExecutionError::from(err)))
                .map_err(|err| err.with_trace(top_trace.clone()))?;

            parent_node
                .flow_data(node, edge_info.source_output, edge_info.target_input)
                .map_err(|err| ErrorWithTrace::from(NodeExecutionError::from(err)))
                .map_err(|err| {
                    err.with_trace({
                        let mut trace = top_trace.clone();
                        trace.highlight_node(parent_id);
                        trace.highlight_node(node_id);
                        trace.highlight_connection(
                            parent_id,
                            edge_info.source_output,
                            node_id,
                            edge_info.target_input,
                        );
                        trace
                    })
                })?;
        }
        Ok(())
    }

    /// Builds a vec of all non-lazy nodes in the graph. On evaluation, these are evaluated in order
    fn urgent_nodes<'s>(
        &'s self,
        registry: &Registry,
    ) -> Result<Vec<&'s NodeIndex>, NodeExecutionError> {
        let mut urgent_nodes = Vec::new();
        for (_, idx) in &self.node_indices {
            let node_id = *self
                .dag
                .node_weight(*idx)
                .ok_or_else(|| NodeExecutionError::from(NodeNotFoundInGraphError::from(*idx)))?;

            let node = match registry.get_node_any(node_id) {
                Some(node) => node,
                None => {
                    return Err(NodeExecutionError::from(NodesNotFoundError::from(
                        &[node_id] as &[NodeReflection],
                    )));
                }
            };
            if node.eval_strategy() == EvalStrategy::Urgent {
                urgent_nodes.push(idx);
            }
        }
        Ok(urgent_nodes)
    }

    fn get_node_id_from_node_index(
        &self,
        idx: NodeIndex,
    ) -> Result<NodeReflection, NodeNotFoundInGraphError> {
        self.dag
            .node_weight(idx)
            .map(|n| *n)
            .ok_or_else(|| NodeNotFoundInGraphError::from(idx))
    }
}
