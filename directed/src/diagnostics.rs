//! Graph traces and mermaid rendering. Kept independent of the engine so it can
//! be tested with plain data.

use std::fmt::Write;

use crate::graph::Graph;
use crate::registry::{NodeId, Registry};

/// A snapshot of a graph, suitable for rendering.
pub struct Trace {
    pub nodes: Vec<TraceNode>,
    pub edges: Vec<TraceEdge>,
}

pub struct TraceNode {
    pub id: NodeId,
    pub name: &'static str,
    pub inputs: Vec<&'static str>,
    pub outputs: Vec<&'static str>,
    pub highlighted: bool,
}

pub struct TraceEdge {
    pub from: NodeId,
    pub from_port: &'static str,
    pub to: NodeId,
    pub to_port: &'static str,
    pub highlighted: bool,
}

impl Graph {
    /// Build a renderable trace, optionally highlighting a set of nodes and
    /// edges.
    pub fn trace(
        &self,
        registry: &Registry,
        highlighted_nodes: &[NodeId],
        highlighted_edges: &[(NodeId, &'static str, NodeId, &'static str)],
    ) -> Trace {
        let nodes = self
            .nodes()
            .iter()
            .filter_map(|&id| {
                let node = registry.lock(id)?;
                let signature = node.signature();
                Some(TraceNode {
                    id,
                    name: signature.stage,
                    inputs: signature.inputs.iter().map(|port| port.name).collect(),
                    outputs: signature.outputs.iter().map(|port| port.name).collect(),
                    highlighted: highlighted_nodes.contains(&id),
                })
            })
            .collect();

        let edges = self
            .edges()
            .iter()
            .map(|edge| TraceEdge {
                from: edge.from,
                from_port: edge.from_port.name,
                to: edge.to,
                to_port: edge.to_port.name,
                highlighted: highlighted_edges
                    .iter()
                    .any(|&(from, from_port, to, to_port)| {
                        from == edge.from
                            && from_port == edge.from_port.name
                            && to == edge.to
                            && to_port == edge.to_port.name
                    }),
            })
            .collect();

        Trace { nodes, edges }
    }
}

impl Trace {
    /// Render the trace as a mermaid `flowchart`. Wrap in a ```mermaid fence.
    pub fn mermaid(&self) -> String {
        const EMPHASIS: &str = "stroke:yellow,stroke-width:3;";
        let mut out = String::new();
        writeln!(out, "flowchart TB").unwrap();

        for node in &self.nodes {
            writeln!(
                out,
                "    subgraph Node_{}[\"Node {} ({})\"]",
                node.id, node.id, node.name
            )
            .unwrap();
            for input in &node.inputs {
                writeln!(
                    out,
                    "        {}_in_{}[/\"{}\"\\]",
                    node.id,
                    sanitize(input),
                    input
                )
                .unwrap();
            }
            for output in &node.outputs {
                writeln!(
                    out,
                    "        {}_out_{}[\\\"{}\"/]",
                    node.id,
                    sanitize(output),
                    output
                )
                .unwrap();
            }
            writeln!(out, "    end").unwrap();
            if node.highlighted {
                writeln!(out, "    style Node_{} {EMPHASIS}", node.id).unwrap();
            }
        }

        for (index, edge) in self.edges.iter().enumerate() {
            writeln!(
                out,
                "    {}_out_{} --> {}_in_{}",
                edge.from,
                sanitize(edge.from_port),
                edge.to,
                sanitize(edge.to_port)
            )
            .unwrap();
            if edge.highlighted {
                writeln!(out, "    linkStyle {index} {EMPHASIS}").unwrap();
            }
        }

        out
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' | '\t' | '-' | '.' | '|' | ':' | '/' | '\\' => '_',
            other => other,
        })
        .collect()
}
