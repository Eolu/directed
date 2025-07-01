#![doc = include_str!("../README.md")]
mod error;
mod graphs;
mod node;
mod registry;
mod stage;

// TODO: Separate out internal-only interfaces
pub use directed_stage_macro::stage;
pub use error::*;
pub use facet;
pub use graphs::{EdgeInfo, Graph};
pub use node::{AnyNode, Cached, DowncastEq, DynFields, Node};
pub use registry::{NodeId, Registry};
pub use stage::{EvalStrategy, ReevaluationRule, RefType, SetVal, Stage, StageShape};

/// Simple macro to simulate a function that can return multiple names outputs
#[macro_export]
macro_rules! output {
    ($($name:ident $(: $val:expr)?),*) => {
        StageOutputType {
            $( $name: output!(@internal $name $(, $val)?), )*
        }
    };
    (@internal $name:ident, $val:expr) => { Some($val) };
    (@internal $name:ident) => { Some($name) };
}

/// Macro to generate the proper state struct
#[macro_export]
macro_rules! state {
    ($stage:ident { $($name:ident $(: $val:expr)?),* }) => {
        TypeAlias::<<$stage as directed::stage::Stage>::State> { $( $name $(: $val)?, )* }
    };
}

/// A simple alias to work around a lack of knowledge in certain contexts.
/// See: https://github.com/rust-lang/rust/issues/86935
pub type TypeAlias<T> = T;

// TODO: Remove this, here to make the IDE complain
// #[test]
// fn basic_macro_test() {
//     extern crate self as directed;
//     use directed_stage_macro::stage;
//     use directed::facet::Facet;

//     #[stage(lazy, cache_last)]
//     fn TinyStage1() -> String {
//         println!("Running stage 1");
//         String::from("This is the output!")
//     }

//     #[stage(lazy, cache_last)]
//     fn TinyStage2(input: String, input2: String) -> String {
//         println!("Running stage 2");
//         input.to_uppercase() + " [" + &input2.chars().count().to_string() + " chars]"
//     }

//     #[stage(cache_last)]
//     fn TinyStage3(input: String) {
//         println!("Running stage 3");
//         assert_eq!("THIS IS THE OUTPUT! [19 chars]", input);
//     }

//     let mut registry = Registry::new();
//     let node_1 = registry.register(TinyStage1);
//     let node_2 = registry.register(TinyStage2);
//     let node_3 = registry.register(TinyStage3);
//     let graph = graph! {
//         nodes: (node_1, node_2, node_3),
//         connections: {
//             node_1 => node_2: input,
//             node_1 => node_2: input2,
//             node_2 => node_3: input,
//         }
//     }
//     .unwrap();

//     graph.execute(&mut registry).unwrap();
// }

#[cfg(test)]
mod tests {
    extern crate self as directed;
    use super::*;
    use directed::facet::Facet;
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
        let node_1 = registry.register(TinyStage1);
        let node_2 = registry.register(TinyStage2);
        let node_3 = registry.register(TinyStage3);
        let graph = graph! {
            nodes: (node_1, node_2, node_3),
            connections: {
                node_1 => node_2: input,
                node_1 => node_2: input2,
                node_2 => node_3: input,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }

    // TODO: Reintroduce this
    // /// Test to verify that the output of a graph can be obtained
    // #[test]
    // fn get_output_test() {
    //     #[stage]
    //     fn TinyStage1() -> String {
    //         println!("Running stage 1");
    //         String::from("This is the output!")
    //     }

    //     let mut registry = Registry::new();
    //     let node_1 = registry.register(TinyStage1);
    //     let graph = graph! {
    //         nodes: (node_1),
    //         connections: {}
    //     }
    //     .unwrap();

    //     graph.execute(&mut registry).unwrap();
    //     let mut outputs = graph.get_output(&mut registry).unwrap();
    //     assert_eq!(
    //         outputs.take_unnamed::<String>(node_1).unwrap(),
    //         String::from("This is the output!")
    //     )
    // }

    // TODO: Reintroduce this
    // /// Test to verify that a grapoh can take inputs
    // #[test]
    // fn inject_input_test() {
    //     #[stage]
    //     fn TinyStage1(simple_input: String) -> String {
    //         println!("Running stage 1");
    //         simple_input.replace("input", "output")
    //     }

    //     let mut registry = Registry::new();
    //     let node_1 = registry.register(TinyStage1::new());
    //     let graph = graph! {
    //         nodes: (node_1),
    //         connections: {}
    //     }
    //     .unwrap();

    //     graph.set_input(&mut registry, node_1, "simple_input", String::from("This is the simple input!")).unwrap();
    //     graph.execute(&mut registry).unwrap();
    //     let mut outputs = graph.get_output(&mut registry).unwrap();
    //     assert_eq!(
    //         outputs.take_unnamed::<String>(node_1).unwrap(),
    //         String::from("This is the simple output!")
    //     )
    // }

    // Test multiple output stages
    #[test]
    fn multiple_output_stage_test() {
        #[stage(out(number: i32, text: String))]
        fn MultiOutputStage() {
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
        let producer = registry.register(MultiOutputStage);
        let consumer1 = registry.register(ConsumerStage1);
        let consumer2 = registry.register(ConsumerStage2);

        let graph = graph! {
            nodes: (producer, consumer1, consumer2),
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
        let lazy_node = registry.register(LazyStage);
        let urgent_node = registry.register(UrgentStage);

        let graph = graph! {
            nodes: (lazy_node, urgent_node),
            connections: {
                lazy_node => urgent_node: input,
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
        let source = registry.register(SourceStage);
        let transparent = registry.register(TransparentStage);
        let opaque = registry.register(OpaqueStage);
        let sink = registry.register(SinkStage);

        let graph = graph! {
            nodes: (source, transparent, opaque, sink),
            connections: {
                source => transparent: input,
                source => opaque: input,
                transparent => sink: t_input,
                opaque => sink: o_input,
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
        let node_a = registry.register(StageA);
        let node_b = registry.register(StageB);

        // Attempt to create a cyclic graph
        let result = graph! {
            nodes: (node_a, node_b),
            connections: {
                node_a => node_b: input,
                node_b => node_a: input,
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
        let node_id = registry.register(SimpleStage);

        // Validate node type
        registry.validate_node_type::<SimpleStage>(node_id.into()).unwrap();

        // Validate incorrect type
        #[stage]
        fn OtherStage() -> String {
            "hello".to_string()
        }
        assert!(registry.validate_node_type::<OtherStage>(node_id.into()).is_err());

        // Get node
        assert!(registry.get(node_id).is_some());

        // Get mutable node
        assert!(registry.get_mut(node_id).is_some());

        // Unregister
        let node = registry
            .unregister::<SimpleStage>(node_id.into())
            .unwrap()
            .unwrap();
        assert!(node.stage.eval_strategy() == EvalStrategy::Urgent);

        // Node no longer exists
        assert!(registry.get(node_id).is_none());
    }

    // // Test error handling when node doesn't exist
    // #[test]
    // fn nonexistent_node_test() {
    //     let mut registry = Registry::new();

    //     // Node ID that doesn't exist
    //     let invalid_id = 9999;

    //     // Various operations should fail
    //     assert!(registry.get(invalid_id).is_none());
    //     assert!(registry.get_mut(invalid_id).is_none());
    //     assert!(registry.unregister_and_drop(invalid_id).is_err());
    // }

    // Test type mismatches in connections
    #[test]
    fn type_mismatch_test() {
        #[stage]
        fn StringStage() -> String {
            "Hello".to_string()
        }

        // TODO: Allow an underscore in name!
        #[stage]
        fn IntegerConsumer(input: i32) {
            // This should never execute due to type mismatch
            panic!("Should not execute");
        }

        let mut registry = Registry::new();
        let producer = registry.register(StringStage);
        let consumer = registry.register(IntegerConsumer);

        // Create graph with type-incompatible connection
        let graph = graph! {
            nodes: (producer, consumer),
            connections: {
                producer => consumer: input,
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
        fn ConsumerStage(input1: i32, input2: String) {
            // This should never execute due to missing input
            panic!("Should not execute");
        }

        #[stage]
        fn ProducerStage() -> i32 {
            42
        }

        let mut registry = Registry::new();
        let producer = registry.register(ProducerStage);
        let consumer = registry.register(ConsumerStage);

        // Only connect one of the required inputs
        let graph = graph! {
            nodes: (producer, consumer),
            connections: {
                producer => consumer: input1,
            }
        }
        .unwrap();

        // Execution should fail due to missing input
        let result = graph.execute(&mut registry);
        assert!(result.is_err());
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
        let source = registry.register(Source);
        let path_a = registry.register(PathA);
        let path_b = registry.register(PathB);
        let sink = registry.register(Sink);

        let graph = graph! {
            nodes: (source, path_a, path_b, sink),
            connections: {
                source => path_a: input,
                source => path_b: input,
                path_a => sink: a,
                path_b => sink: b,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
    }

    // // Test accessing outputs by wrong name
    // #[test]
    // fn invalid_output_name_test() {
    //     #[stage]
    //     fn MultiOutputStage() -> NodeOutput {
    //         output! {
    //             output1: 42,
    //             output2: "Hello".to_string()
    //         }
    //     }

    //     #[stage]
    //     fn ConsumerStage(_input: i32) {
    //         // Should never execute
    //         panic!("Should not execute");
    //     }

    //     let mut registry = Registry::new();
    //     let producer = registry.register(MultiOutputStage::new());
    //     let consumer = registry.register(ConsumerStage::new());

    //     // Connect with non-existent output name
    //     let graph = graph! {
    //         nodes: (producer, consumer),
    //         connections: {
    //             producer: nonexistent => consumer: input,
    //         }
    //     }
    //     .unwrap();

    //     // Should fail because the output name doesn't exist
    //     let result = graph.execute(&mut registry);
    //     assert!(result.is_err());
    // }

    /// Test nodes with internal state
    #[test]
    fn node_with_state_test() {
        #[stage(state(state: (u8, u8)))]
        fn StateStage() {
            assert_eq!(state.1, state.0 * 5);
            state.0 += 1;
            state.1 += 5;
            println!("State is {}", state.1);
        }

        let mut registry = Registry::new();
        // Note: If the state has an implementation of "default", the simple
        // register can still be called instead
        // TODO: This is super convoluted and gross, find a nicer way
        let node = registry.register_with_state(StateStage, state!(StateStage { state: (1, 5) }));
        let graph = graph! {
            nodes: (node),
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
        fn ConsumeOutputs(num: i32, txt: String, vect: Vec<i32>) {
            assert_eq!(num, 42);
            assert_eq!(txt, "hello");
            assert_eq!(vect, vec![1, 2, 3]);
        }

        let mut registry = Registry::new();
        let producer = registry.register(ProduceOutput1);
        let consumer = registry.register(ConsumeOutputs);

        let graph = graph! {
            nodes: (producer, consumer),
            connections: {
                producer: number => consumer: num,
                producer: text => consumer: txt,
                producer: vector => consumer: vect,
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
        let node_a = registry.register(StageA);

        // Correct type validation should succeed
        assert!(registry.validate_node_type::<StageA>(node_a.into()).is_ok());

        // Incorrect type validation should fail
        assert!(registry.validate_node_type::<StageB>(node_a.into()).is_err());

        // Unregistering with incorrect type should fail
        assert!(registry.unregister::<StageB>(node_a.into()).is_err());

        // Unregistering with correct type should succeed
        assert!(registry.unregister::<StageA>(node_a.into()).is_ok());
    }

    // #[test]
    // fn basic_cache_all_test() {
    //     static COUNTER: AtomicUsize = AtomicUsize::new(0);

    //     #[stage(lazy, cache_all)]
    //     fn CacheStage1() -> String {
    //         println!("Running stage 1");
    //         COUNTER.fetch_add(1, Ordering::SeqCst);
    //         String::from("This is the output!")
    //     }

    //     #[stage(lazy, cache_all)]
    //     fn CacheStage2(input: String, input2: String) -> String {
    //         println!("Running stage 2");
    //         COUNTER.fetch_add(1, Ordering::SeqCst);
    //         input.to_uppercase() + " [" + &input2.chars().count().to_string() + " chars]"
    //     }

    //     #[stage(cache_last)]
    //     fn TinyStage3(input: String) {
    //         println!("Running stage 3");
    //         assert_eq!("THIS IS THE OUTPUT! [19 chars]", input);
    //     }

    //     #[stage(lazy, cache_all)]
    //     fn CacheStage1Alternate() -> String {
    //         println!("Running alt stage 1");
    //         COUNTER.fetch_add(1, Ordering::SeqCst);
    //         String::from("This is a different output!")
    //     }

    //     #[stage(cache_last)]
    //     fn TinyStage3Alternate(input: String) {
    //         println!("Running alt stage 3");
    //         assert_eq!("THIS IS A DIFFERENT OUTPUT! [27 chars]", input);
    //     }

    //     let mut registry = Registry::new();
    //     let node_1 = registry.register(CacheStage1);
    //     let node_2 = registry.register(CacheStage2);
    //     let node_3 = registry.register(TinyStage3);
    //     let node_1_alt = registry.register(CacheStage1Alternate);
    //     let node_3_alt = registry.register(TinyStage3Alternate);

    //     let graph1 = graph! {
    //         nodes: (node_1, node_2, node_3),
    //         connections: {
    //             node_1 => node_2: input,
    //             node_1 => node_2: input2,
    //             node_2 => node_3: input,
    //         }
    //     }
    //     .unwrap();

    //     graph1.execute(&mut registry).unwrap();
    //     assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
    //     graph1.execute(&mut registry).unwrap();
    //     assert_eq!(COUNTER.load(Ordering::SeqCst), 2);

    //     // Now with a modified graph, but same stage 2
    //     let graph2 = graph! {
    //         nodes: (node_1_alt, node_2, node_3_alt),
    //         connections: {
    //             node_1_alt => node_2: input,
    //             node_1_alt => node_2: input2,
    //             node_2 => node_3_alt: input,
    //         }
    //     }
    //     .unwrap();

    //     graph2.execute(&mut registry).unwrap();
    //     assert_eq!(COUNTER.load(Ordering::SeqCst), 4);
    //     graph2.execute(&mut registry).unwrap();
    //     assert_eq!(COUNTER.load(Ordering::SeqCst), 4);
    //     graph1.execute(&mut registry).unwrap();
    //     assert_eq!(COUNTER.load(Ordering::SeqCst), 4);
    // }

    /// Test connections without data
    #[test]
    fn blank_connections_test() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        #[stage(lazy)]
        fn TinyStage1() {
            println!("Running stage 1");
            COUNTER.fetch_add(1, Ordering::SeqCst);
        }

        #[stage(lazy)]
        fn TinyStage2() {
            println!("Running stage 2");
            assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
            COUNTER.fetch_add(1, Ordering::SeqCst);
        }

        #[stage]
        fn TinyStage3() {
            println!("Running stage 3");
            assert_eq!(COUNTER.load(Ordering::SeqCst), 3);
            COUNTER.fetch_add(1, Ordering::SeqCst);
        }

        let mut registry = Registry::new();
        let node_1 = registry.register(TinyStage1);
        let node_2 = registry.register(TinyStage2);
        let node_3 = registry.register(TinyStage3);
        let graph = graph! {
            nodes: (node_1, node_2, node_3),
            connections: {
                node_1 => node_2,
                node_2 => node_3,
                node_1 => node_3,
            }
        }
        .unwrap();

        graph.execute(&mut registry).unwrap();
        assert_eq!(COUNTER.load(Ordering::SeqCst), 4);
    }

    // TODO: Specific test for trace generation
}

// In src/lib.rs - Add a test for async execution
#[cfg(all(test, feature = "tokio"))]
mod async_tests {
    extern crate self as directed;
    use super::*;
    use directed_stage_macro::stage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn parallel_execution_test() {
        use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let (tx1, rx1) = unbounded_channel::<u8>();
        let (tx2, rx2) = unbounded_channel::<u8>();

        #[stage(lazy, state(tx: UnboundedSender<u8>, rx: UnboundedReceiver<u8>))]
        async fn SlowStage1() -> i32 {
            println!("Running SlowStage1");
            tx.send(1).unwrap();
            assert_eq!(rx.recv().await.unwrap(), 2);
            COUNTER.fetch_add(1, Ordering::SeqCst);
            42
        }

        #[stage(lazy, state(tx: UnboundedSender<u8>, rx: UnboundedReceiver<u8>))]
        async fn SlowStage2() -> String {
            println!("Running SlowStage2");
            assert_eq!(rx.recv().await.unwrap(), 1);
            tx.send(2).unwrap();
            COUNTER.fetch_add(1, Ordering::SeqCst);
            "hello".to_string()
        }

        #[stage]
        fn CombineStage(as_num: i32, as_text: String) -> String {
            println!("Running CombineStage");
            format!("{} {}", as_text, as_num)
        }

        let mut registry = Registry::new();
        let stage1 = registry.register_with_state(SlowStage1::new(), (tx1, rx2));
        let stage2 = registry.register_with_state(SlowStage2::new(), (tx2, rx1));
        let combine = registry.register(CombineStage::new());

        let graph = graph! {
            nodes: (stage1, stage2, combine),
            connections: {
                stage1 => combine: as_num,
                stage2 => combine: as_text,
            }
        }
        .unwrap();
        let graph = std::sync::Arc::new(graph);

        // Reset counter
        COUNTER.store(0, Ordering::SeqCst);

        graph
            .execute_async(tokio::sync::Mutex::new(registry))
            .await
            .unwrap();

        // Both stages should have been executed
        assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
    }
}
