#![doc = include_str!("../README.md")]
mod graphs;
mod node;
mod registry;
mod stage;
mod types;
mod error;

pub use directed_stage_macro::stage;
pub use graphs::{EdgeInfo, Graph};
pub use node::{AnyNode, Node};
pub use registry::Registry;
pub use stage::{EvalStrategy, ReevaluationRule, RefType, Stage};
pub use types::{DataLabel, NodeOutput};
pub use error::*;

#[cfg(test)]
mod tests {
    extern crate self as directed;
    use super::*;
    use directed_stage_macro::stage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A simple sanity-check test that doesn't try anything interesting
    #[test]
    fn basic_macro_test() {
        #[stage(lazy, cache_last)]
        fn TinyStage1() -> String {
            println!("Running stage 1");
            String::from("This is the output!")
        }

        #[stage(lazy, cache_last)]
        fn TinyStage2(input: String, input2: String) -> String {
            println!("Running stage 2");
            input.to_uppercase() + " [" + &input2.chars().count().to_string() + " chars]"
        }

        #[stage(cache_last)]
        fn TinyStage3(input: String) {
            println!("Running stage 3");
            assert_eq!("THIS IS THE OUTPUT! [19 chars]", input);
        }

        let mut registry = Registry::new();
        let node_1 = registry.register(TinyStage1::new());
        let node_2 = registry.register(TinyStage2::new());
        let node_3 = registry.register(TinyStage3::new());
        let graph = graph! {
            nodes: [node_1, node_2, node_3],
            connections: {
                node_1: _ => node_2: input,
                node_1: _ => node_2: input2,
                node_2: _ => node_3: input,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }

    // Test multiple output stages
    #[test]
    fn multiple_output_stage_test() {
        #[stage(out(number: i32, text: String))]
        fn MultiOutputStage() -> NodeOutput {
            let value1 = 42;
            let value2 = String::from("Hello");
            output! {
                number: value1,
                text: value2
            }
        }

        #[stage]
        fn ConsumerStage1(number: i32) {
            assert_eq!(number, 42);
        }

        #[stage]
        fn ConsumerStage2(text: String) {
            assert_eq!(text, "Hello");
        }

        let mut registry = Registry::new();
        let producer = registry.register(MultiOutputStage::new());
        let consumer1 = registry.register(ConsumerStage1::new());
        let consumer2 = registry.register(ConsumerStage2::new());

        let graph = graph! {
            nodes: [producer, consumer1, consumer2],
            connections: {
                producer: number => consumer1: number,
                producer: text => consumer2: text,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }

    // Test evaluating lazy vs urgent nodes
    #[test]
    fn lazy_and_urgent_eval_test() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        #[stage(lazy, cache_last)]
        fn LazyStage() -> i32 {
            COUNTER.fetch_add(1, Ordering::SeqCst);
            42
        }

        #[stage(cache_last)]
        fn UrgentStage(input: i32) {
            assert_eq!(input, 42);
            assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
        }

        let mut registry = Registry::new();
        let lazy_node = registry.register(LazyStage::new());
        let urgent_node = registry.register(UrgentStage::new());

        let graph = graph! {
            nodes: [lazy_node, urgent_node],
            connections: {
                lazy_node: _ => urgent_node: input,
            }
        }
        .unwrap();

        // Reset counter
        COUNTER.store(0, Ordering::SeqCst);

        // Execute should evaluate LazyStage because UrgentStage depends on it
        graph.execute(&mut registry).unwrap();
    }

    // Test transparent vs opaque reevaluation rules
    #[test]
    fn transparent_opaque_reevaluation_test() {
        static TRANSPARENT_COUNTER: AtomicUsize = AtomicUsize::new(0);
        static OPAQUE_COUNTER: AtomicUsize = AtomicUsize::new(0);

        #[stage(lazy, cache_last)]
        fn SourceStage() -> i32 {
            println!("SourceStage");
            42
        }

        #[stage(lazy, cache_last)]
        fn TransparentStage(input: i32) -> i32 {
            println!("TransparentStage");
            TRANSPARENT_COUNTER.fetch_add(1, Ordering::SeqCst);
            input * 2
        }

        #[stage(lazy)]
        fn OpaqueStage(input: &i32) -> i32 {
            println!("OpaqueStage");
            OPAQUE_COUNTER.fetch_add(1, Ordering::SeqCst);
            input * 3
        }

        #[stage]
        fn SinkStage(t_input: &i32, o_input: &i32) {
            println!("SinkStage");
            assert_eq!(*t_input, 84);
            assert_eq!(*o_input, 126);
        }

        let mut registry = Registry::new();
        let source = registry.register(SourceStage::new());
        let transparent = registry.register(TransparentStage::new());
        let opaque = registry.register(OpaqueStage::new());
        let sink = registry.register(SinkStage::new());

        let graph = graph! {
            nodes: [source, transparent, opaque, sink],
            connections: {
                source: _ => transparent: input,
                source: _ => opaque: input,
                transparent: _ => sink: t_input,
                opaque: _ => sink: o_input,
            }
        }
        .unwrap();

        // Reset counters
        TRANSPARENT_COUNTER.store(0, Ordering::SeqCst);
        OPAQUE_COUNTER.store(0, Ordering::SeqCst);

        // First execution
        graph.execute(&mut registry).unwrap();
        assert_eq!(TRANSPARENT_COUNTER.load(Ordering::SeqCst), 1);
        assert_eq!(OPAQUE_COUNTER.load(Ordering::SeqCst), 1);

        // Second execution - transparent stage shouldn't execute again since inputs haven't changed
        graph.execute(&mut registry).unwrap();
        assert_eq!(TRANSPARENT_COUNTER.load(Ordering::SeqCst), 1); // Still 1
        assert_eq!(OPAQUE_COUNTER.load(Ordering::SeqCst), 2); // Increased to 2

        println!("{}", graph.generate_trace(&registry, vec![sink], vec![(opaque, "_".into(), sink, "o_input".into())]).create_mermaid_graph());
    }

    // Test graph cycle detection
    #[test]
    fn cycle_detection_test() {
        #[stage]
        fn StageA(input: i32) -> i32 {
            input + 1
        }

        #[stage]
        fn StageB(input: i32) -> i32 {
            input * 2
        }

        let mut registry = Registry::new();
        let node_a = registry.register(StageA::new());
        let node_b = registry.register(StageB::new());

        // Attempt to create a cyclic graph
        let result = graph! {
            nodes: [node_a, node_b],
            connections: {
                node_a: _ => node_b: input,
                node_b: _ => node_a: input,
            }
        };

        // The graph creation should fail due to cycle detection
        assert!(result.is_err());
    }

    // Test registry functionality
    #[test]
    fn registry_operations_test() {
        #[stage]
        fn SimpleStage() -> i32 {
            42
        }

        let mut registry = Registry::new();

        // Register a node
        let node_id = registry.register(SimpleStage::new());

        // Validate node type
        registry.validate_node_type::<SimpleStage>(node_id).unwrap();

        // Validate incorrect type
        #[stage]
        fn OtherStage() -> String {
            "hello".to_string()
        }
        assert!(registry.validate_node_type::<OtherStage>(node_id).is_err());

        // Get node
        assert!(registry.get(node_id).is_some());

        // Get mutable node
        assert!(registry.get_mut(node_id).is_some());

        // Unregister
        let node = registry
            .unregister::<SimpleStage>(node_id)
            .unwrap()
            .unwrap();
        assert!(node.stage.eval_strategy() == EvalStrategy::Urgent);

        // Node no longer exists
        assert!(registry.get(node_id).is_none());
    }

    // Test error handling when node doesn't exist
    #[test]
    fn nonexistent_node_test() {
        let mut registry = Registry::new();

        // Node ID that doesn't exist
        let invalid_id = 9999;

        // Various operations should fail
        assert!(registry.get(invalid_id).is_none());
        assert!(registry.get_mut(invalid_id).is_none());
        assert!(registry.unregister_and_drop(invalid_id).is_err());
    }

    // Test type mismatches in connections
    #[test]
    fn type_mismatch_test() {
        #[stage]
        fn StringStage() -> String {
            "Hello".to_string()
        }

        #[stage]
        fn IntegerConsumer(_input: i32) {
            // This should never execute due to type mismatch
            panic!("Should not execute");
        }

        let mut registry = Registry::new();
        let producer = registry.register(StringStage::new());
        let consumer = registry.register(IntegerConsumer::new());

        // Create graph with type-incompatible connection
        let graph = graph! {
            nodes: [producer, consumer],
            connections: {
                producer: _ => consumer: input,
            }
        }
        .unwrap();

        // Execution should fail due to type mismatch when flowing data
        let result = graph.execute(&mut registry);
        assert!(result.is_err());
    }

    // Test missing inputs
    #[test]
    fn missing_input_test() {
        #[stage]
        fn ConsumerStage(_input1: i32, _input2: String) {
            // This should never execute due to missing input
            panic!("Should not execute");
        }

        #[stage]
        fn ProducerStage() -> i32 {
            42
        }

        let mut registry = Registry::new();
        let producer = registry.register(ProducerStage::new());
        let consumer = registry.register(ConsumerStage::new());

        // Only connect one of the required inputs
        let graph = graph! {
            nodes: [producer, consumer],
            connections: {
                producer: _ => consumer: input1,
            }
        }
        .unwrap();

        // Execution should fail due to missing input
        let result = graph.execute(&mut registry);
        assert!(result.is_err());
    }

    // Test DataLabel functionality
    #[test]
    fn data_label_test() {
        let label1 = DataLabel::new("test");
        let label2 = DataLabel::new("test");
        let label3 = DataLabel::new("different");

        assert_eq!(label1, label2);
        assert_ne!(label1, label3);

        let const_label = DataLabel::new_const("const");
        assert_eq!(const_label.inner(), "const");

        let from_str: DataLabel = "string".into();
        assert_eq!(from_str.inner(), "string");
    }

    // Test graph with diamond pattern
    #[test]
    fn diamond_graph_test() {
        #[stage]
        fn Source() -> i32 {
            10
        }

        #[stage]
        fn PathA(input: i32) -> i32 {
            input * 2
        }

        #[stage]
        fn PathB(input: i32) -> i32 {
            input + 5
        }

        #[stage]
        fn Sink(a: i32, b: i32) {
            assert_eq!(a, 20); // 10 * 2
            assert_eq!(b, 15); // 10 + 5
        }

        let mut registry = Registry::new();
        let source = registry.register(Source::new());
        let path_a = registry.register(PathA::new());
        let path_b = registry.register(PathB::new());
        let sink = registry.register(Sink::new());

        let graph = graph! {
            nodes: [source, path_a, path_b, sink],
            connections: {
                source: _ => path_a: input,
                source: _ => path_b: input,
                path_a: _ => sink: a,
                path_b: _ => sink: b,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }

    // Test accessing outputs by wrong name
    #[test]
    fn invalid_output_name_test() {
        #[stage]
        fn MultiOutputStage() -> NodeOutput {
            output! {
                output1: 42,
                output2: "Hello".to_string()
            }
        }

        #[stage]
        fn ConsumerStage(_input: i32) {
            // Should never execute
            panic!("Should not execute");
        }

        let mut registry = Registry::new();
        let producer = registry.register(MultiOutputStage::new());
        let consumer = registry.register(ConsumerStage::new());

        // Connect with non-existent output name
        let graph = graph! {
            nodes: [producer, consumer],
            connections: {
                producer: nonexistent => consumer: input,
            }
        }
        .unwrap();

        // Should fail because the output name doesn't exist
        let result = graph.execute(&mut registry);
        assert!(result.is_err());
    }

    /// Test nodes with internal state
    #[test]
    fn node_with_state_test() {
        #[stage(state((u8, u8)))]
        fn StateStage() {
            assert_eq!(state.1, state.0 * 5);
            state.0 += 1;
            state.1 += 5;
            println!("State is {}", state.1);
        }

        let mut registry = Registry::new();
        // Note: If the state has an implementation of "default", the simple
        // register can still be called instead
        let node = registry.register_with_state(StateStage::new(), (1, 5));
        let graph = graph! {
            nodes: [node],
            connections: {}
        }
        .unwrap();

        // TODO: Actually return results so this test can be real (right now it would pass if state never updated)
        graph.execute(&mut registry).unwrap();
        graph.execute(&mut registry).unwrap();
        graph.execute(&mut registry).unwrap();
        graph.execute(&mut registry).unwrap();
    }

    // Test the output! macro
    #[test]
    fn output_macro_test() {
        #[stage(out(number: i32, text: String, vector: Vec<i32>))]
        fn ProduceOutput1() -> NodeOutput {
            println!("Running ProduceOutput1");
            let number = 42;
            let text = "hello".to_string();
            let vector = vec![1, 2, 3];

            output! {
                number,
                text,
                vector
            }
        }

        #[stage]
        fn ConsumeOutputs(num: i32, txt: String, vec: Vec<i32>) {
            assert_eq!(num, 42);
            assert_eq!(txt, "hello");
            assert_eq!(vec, vec![1, 2, 3]);
        }

        let mut registry = Registry::new();
        let producer = registry.register(ProduceOutput1::new());
        let consumer = registry.register(ConsumeOutputs::new());

        let graph = graph! {
            nodes: [producer, consumer],
            connections: {
                producer: number => consumer: num,
                producer: text => consumer: txt,
                producer: vector => consumer: vec,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }

    // Test registry node type validation
    #[test]
    fn registry_type_validation_test() {
        #[stage]
        fn StageA() -> i32 {
            42
        }

        #[stage]
        fn StageB() -> String {
            "hello".to_string()
        }

        let mut registry = Registry::new();
        let node_a = registry.register(StageA::new());

        // Correct type validation should succeed
        assert!(registry.validate_node_type::<StageA>(node_a).is_ok());

        // Incorrect type validation should fail
        assert!(registry.validate_node_type::<StageB>(node_a).is_err());

        // Unregistering with incorrect type should fail
        assert!(registry.unregister::<StageB>(node_a).is_err());

        // Unregistering with correct type should succeed
        assert!(registry.unregister::<StageA>(node_a).is_ok());
    }
}
