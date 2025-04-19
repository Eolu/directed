//! TODO: Top-level docs
mod graphs;
mod node;
mod registry;
mod stage;
mod types;

// TODO: Make a cool visual "rust playground" based on this crate
//    - Ability to create stages, and compile
//    - Ability to create nodes from stages, and attach them and execute (without recompiling!)

// TODO: Make sure public interface structure makes sense
pub use directed_macros::*;
pub use graphs::{EdgeInfo, Graph};
pub use node::{AnyNode, Node};
pub use registry::Registry;
pub use stage::{EvalStrategy, ReevaluationRule, Stage};
pub use types::{DataLabel, NodeOutput};

// TODO: Add the ability to take inputs by reference in transparent stage (should be the correct, default behavior)
// TODO: Graph + Registry could be combined to create a Node (with a baked stage)
// TODO: An attribute that makes it serialize the cache and store between runs!
// TODO: Accept inputs for top-level nodes, return outputs from leaf nodes
// TODO: Automatic validators to make sure correct input and output types are present if required
// TODO: Caching ALL possible input combinations, not just previous: transparent(cache_all)
// TODO: A way to reset all registry state at once
// TODO: A canon way to create a stage out of an entire graph

#[cfg(test)]
mod tests {
    extern crate self as directed;
    use directed_macros::stage;

    use super::*;

    /// Initial test to prove a simple pipeline works
    #[test]
    fn basic_macro_test() {
        #[stage(lazy, transparent)]
        fn TinyStage1() -> String {
            println!("Running stage 1");
            String::from("This is the output!")
        }

        #[stage(lazy, transparent)]
        fn TinyStage2(input: String, input2: String) -> String {
            println!("Running stage 2");
            input.to_uppercase() + &input2.to_lowercase()
        }

        #[stage(transparent)]
        fn TinyStage3(input: String) {
            println!("Running stage 3");
            assert_eq!("THIS IS THE OUTPUT!this is the output!", input);
        }

        let mut registry = Registry::new();
        let node_1 = registry.register(TinyStage1::new());
        let node_2 = registry.register(TinyStage2::new());
        let node_3 = registry.register(TinyStage3::new());
        let graph = graph! {
            nodes: &[node_1, node_2, node_3],
            // TODO: The "_" for an unnamed return is not great, this can be made better
            connections: {
                node_1 => "_" => "input" => node_2,
                node_1 => "_" => "input2" => node_2,
                node_2 => "_" => "input" => node_3,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }
}
