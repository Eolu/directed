use std::sync::atomic::{AtomicUsize, Ordering};

use directed::{GraphBuilder, Registry, StageHandle};

#[test]
fn basic_macro_test() {
    #[directed::stage(lazy, cache_last)]
    fn TinyStage1() -> String {
        String::from("This is the output!")
    }

    #[directed::stage(lazy, cache_last)]
    fn TinyStage2(input: String, _input2: String) -> String {
        input.to_uppercase() + " [" + &input.chars().count().to_string() + " chars]"
    }

    #[directed::stage(cache_last)]
    fn TinyStage3(input: String) {
        assert_eq!("THIS IS THE OUTPUT! [19 chars]", input);
    }

    let mut registry = Registry::new();
    let node_1 = registry.register::<TinyStage1>();
    let node_2 = registry.register::<TinyStage2>();
    let node_3 = registry.register::<TinyStage3>();
    let graph = directed::graph! {
        nodes: [node_1, node_2, node_3],
        connections: {
            node_1: out => node_2: input,
            node_1: out => node_2: input2,
            node_2: out => node_3: input,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn multiple_output_stage_test() {
    #[directed::stage(out(number: i32, text: String))]
    fn MultiOutputStage() -> (i32, String) {
        (42, String::from("Hello"))
    }

    #[directed::stage]
    fn ConsumerStage1(number: i32) {
        assert_eq!(number, 42);
    }

    #[directed::stage]
    fn ConsumerStage2(text: String) {
        assert_eq!(text, "Hello");
    }

    let mut registry = Registry::new();
    let producer = registry.register::<MultiOutputStage>();
    let consumer1 = registry.register::<ConsumerStage1>();
    let consumer2 = registry.register::<ConsumerStage2>();

    let graph = directed::graph! {
        nodes: [producer, consumer1, consumer2],
        connections: {
            producer: number => consumer1: number,
            producer: text => consumer2: text,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn lazy_and_urgent_eval_test() {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    COUNTER.store(0, Ordering::SeqCst);

    #[directed::stage(lazy, cache_last)]
    fn LazyStage() -> i32 {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        42
    }

    #[directed::stage(cache_last)]
    fn UrgentStage(input: i32) {
        assert_eq!(input, 42);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    }

    let mut registry = Registry::new();
    let lazy_node = registry.register::<LazyStage>();
    let urgent_node = registry.register::<UrgentStage>();

    let graph = directed::graph! {
        nodes: [lazy_node, urgent_node],
        connections: {
            lazy_node: out => urgent_node: input,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn transparent_opaque_reevaluation_test() {
    static TRANSPARENT: AtomicUsize = AtomicUsize::new(0);
    static OPAQUE: AtomicUsize = AtomicUsize::new(0);
    TRANSPARENT.store(0, Ordering::SeqCst);
    OPAQUE.store(0, Ordering::SeqCst);

    #[directed::stage(lazy, cache_last)]
    fn SourceStage() -> i32 {
        42
    }

    #[directed::stage(lazy, cache_last)]
    fn TransparentStage(input: i32) -> i32 {
        TRANSPARENT.fetch_add(1, Ordering::SeqCst);
        input * 2
    }

    #[directed::stage(lazy)]
    fn OpaqueStage(input: &i32) -> i32 {
        OPAQUE.fetch_add(1, Ordering::SeqCst);
        input * 3
    }

    #[directed::stage]
    fn SinkStage(t_input: &i32, o_input: &i32) {
        assert_eq!(*t_input, 84);
        assert_eq!(*o_input, 126);
    }

    let mut registry = Registry::new();
    let source = registry.register::<SourceStage>();
    let transparent = registry.register::<TransparentStage>();
    let opaque = registry.register::<OpaqueStage>();
    let sink = registry.register::<SinkStage>();

    let graph = directed::graph! {
        nodes: [source, transparent, opaque, sink],
        connections: {
            source: out => transparent: input,
            source: out => opaque: input,
            transparent: out => sink: t_input,
            opaque: out => sink: o_input,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
    assert_eq!(TRANSPARENT.load(Ordering::SeqCst), 1);
    assert_eq!(OPAQUE.load(Ordering::SeqCst), 1);

    graph.execute(&registry).unwrap();
    assert_eq!(TRANSPARENT.load(Ordering::SeqCst), 1);
    assert_eq!(OPAQUE.load(Ordering::SeqCst), 2);
}

#[test]
fn diamond_graph_test() {
    #[directed::stage]
    fn Source() -> i32 {
        10
    }

    #[directed::stage]
    fn PathA(input: i32) -> i32 {
        input * 2
    }

    #[directed::stage]
    fn PathB(input: i32) -> i32 {
        input + 5
    }

    #[directed::stage]
    fn Sink(a: i32, b: i32) {
        assert_eq!(a, 20);
        assert_eq!(b, 15);
    }

    let mut registry = Registry::new();
    let source = registry.register::<Source>();
    let path_a = registry.register::<PathA>();
    let path_b = registry.register::<PathB>();
    let sink = registry.register::<Sink>();

    let graph = directed::graph! {
        nodes: [source, path_a, path_b, sink],
        connections: {
            source: out => path_a: input,
            source: out => path_b: input,
            path_a: out => sink: a,
            path_b: out => sink: b,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn cycle_detection_test() {
    #[directed::stage]
    fn StageA(input: i32) -> i32 {
        input + 1
    }

    #[directed::stage]
    fn StageB(input: i32) -> i32 {
        input * 2
    }

    let mut registry = Registry::new();
    let node_a = registry.register::<StageA>();
    let node_b = registry.register::<StageB>();

    let result = directed::graph! {
        nodes: [node_a, node_b],
        connections: {
            node_a: out => node_b: input,
            node_b: out => node_a: input,
        }
    };
    assert!(result.is_err());
}

#[test]
fn missing_input_test() {
    #[directed::stage]
    fn ConsumerStage(_input1: i32, _input2: String) {
        panic!("should not execute");
    }

    #[directed::stage]
    fn ProducerStage() -> i32 {
        42
    }

    let mut registry = Registry::new();
    let producer = registry.register::<ProducerStage>();
    let consumer = registry.register::<ConsumerStage>();

    let graph = directed::graph! {
        nodes: [producer, consumer],
        connections: {
            producer: out => consumer: input1,
        }
    }
    .unwrap();

    assert!(graph.execute(&registry).is_err());
}

#[test]
fn invalid_output_name_test() {
    #[directed::stage]
    fn Producer() -> i32 {
        42
    }

    #[directed::stage]
    fn Consumer(_input: i32) {}

    let mut registry = Registry::new();
    let producer = registry.register::<Producer>();
    let consumer = registry.register::<Consumer>();

    let mut builder = GraphBuilder::new();
    builder.add(&producer);
    builder.add(&consumer);
    let result = builder.connect_by_name(
        producer.id(),
        "nonexistent",
        consumer.id(),
        "input",
        &registry,
    );
    assert!(result.is_err());
}

#[test]
fn node_with_state_test() {
    #[directed::stage(state((u8, u8)))]
    fn StateStage() {
        assert_eq!(state.1, state.0 * 5);
        state.0 += 1;
        state.1 += 5;
    }

    let mut registry = Registry::new();
    let node = registry.register_with_state::<StateStage>((1, 5));
    let graph = directed::graph! {
        nodes: [node],
        connections: {}
    }
    .unwrap();

    graph.execute(&registry).unwrap();
    graph.execute(&registry).unwrap();
    graph.execute(&registry).unwrap();
    graph.execute(&registry).unwrap();
}

#[test]
fn registry_operations_test() {
    #[directed::stage]
    fn SimpleStage() -> i32 {
        42
    }

    let mut registry = Registry::new();
    let node = registry.register::<SimpleStage>();
    assert_eq!(registry.len(), 1);
    assert!(registry.unregister(node.id()));
    assert_eq!(registry.len(), 0);
}

#[test]
fn output_values_test() {
    #[directed::stage]
    fn Producer() -> i32 {
        7
    }

    #[directed::stage]
    fn Sink(input: i32) {
        assert_eq!(input, 7);
    }

    let mut registry = Registry::new();
    let producer = registry.register::<Producer>();
    let sink = registry.register::<Sink>();
    let producer_id = producer.id();

    let graph = directed::graph! {
        nodes: [producer, sink],
        connections: {
            producer: out => sink: input,
        }
    }
    .unwrap();

    let outputs = graph.execute(&registry).unwrap();
    // The sink is the only urgent target.
    assert!(outputs.get::<()>(sink.id(), 0).is_some());
    assert!(producer_id < usize::MAX);
}

#[test]
fn cache_all_test() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    CALLS.store(0, Ordering::SeqCst);

    #[directed::stage(lazy)]
    fn Counted() -> i32 {
        5
    }

    #[directed::stage(lazy, cache_all)]
    fn Cached(input: i32) -> i32 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        input * 2
    }

    #[directed::stage]
    fn CacheSink(value: i32) {
        assert_eq!(value, 10);
    }

    let mut registry = Registry::new();
    let counted = registry.register::<Counted>();
    let cached = registry.register::<Cached>();
    let sink = registry.register::<CacheSink>();

    let graph = directed::graph! {
        nodes: [counted, cached, sink],
        connections: {
            counted: out => cached: input,
            cached: out => sink: value,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
    graph.execute(&registry).unwrap();
    graph.execute(&registry).unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn mermaid_trace_test() {
    #[directed::stage]
    fn Producer() -> i32 {
        1
    }

    #[directed::stage]
    fn Sink(input: i32) {
        assert_eq!(input, 1);
    }

    let mut registry = Registry::new();
    let producer = registry.register::<Producer>();
    let sink = registry.register::<Sink>();

    let graph = directed::graph! {
        nodes: [producer, sink],
        connections: {
            producer: out => sink: input,
        }
    }
    .unwrap();

    let trace = graph.trace(
        &registry,
        &[sink.id()],
        &[(producer.id(), "out", sink.id(), "input")],
    );
    let mermaid = trace.mermaid();
    assert!(mermaid.contains("flowchart TB"));
    assert!(mermaid.contains("Producer"));
    assert!(mermaid.contains("Sink"));
    assert!(mermaid.contains("style Node_"));
    assert!(mermaid.contains("linkStyle 0"));
}

#[test]
fn async_stage_test() {
    #[directed::stage]
    async fn AsyncSource() -> i32 {
        21
    }

    #[directed::stage]
    async fn AsyncDouble(input: i32) -> i32 {
        input * 2
    }

    #[directed::stage(cache_last)]
    async fn AsyncSink(input: i32) {
        assert_eq!(input, 42);
    }

    let mut registry = Registry::new();
    let source = registry.register::<AsyncSource>();
    let double = registry.register::<AsyncDouble>();
    let sink = registry.register::<AsyncSink>();

    let graph = directed::graph! {
        nodes: [source, double, sink],
        connections: {
            source: out => double: input,
            double: out => sink: input,
        }
    }
    .unwrap();

    let sink_id = sink.id();
    let outputs = directed::block_on(graph.execute_async(&registry, &[sink_id])).unwrap();
    assert!(outputs.get::<()>(sink_id, 0).is_some());
}

/// Yields once (returns `Pending` a single time) to give the scheduler a chance
/// to poll other ready nodes.
async fn yield_once() {
    let mut polled = false;
    std::future::poll_fn(move |cx| {
        if polled {
            std::task::Poll::Ready(())
        } else {
            polled = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await
}

#[test]
fn independent_nodes_run_concurrently() {
    static ACTIVE: AtomicUsize = AtomicUsize::new(0);
    static MAX_ACTIVE: AtomicUsize = AtomicUsize::new(0);
    ACTIVE.store(0, Ordering::SeqCst);
    MAX_ACTIVE.store(0, Ordering::SeqCst);

    #[directed::stage(lazy)]
    async fn Worker() -> i32 {
        let active = ACTIVE.fetch_add(1, Ordering::SeqCst) + 1;
        MAX_ACTIVE.fetch_max(active, Ordering::SeqCst);
        yield_once().await;
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
        1
    }

    #[directed::stage]
    fn Join(a: i32, b: i32) {
        assert_eq!(a + b, 2);
    }

    let mut registry = Registry::new();
    let left = registry.register::<Worker>();
    let right = registry.register::<Worker>();
    let join = registry.register::<Join>();

    let graph = directed::graph! {
        nodes: [left, right, join],
        connections: {
            left: out => join: a,
            right: out => join: b,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
    // Both independent workers were in flight at the same time.
    assert_eq!(MAX_ACTIVE.load(Ordering::SeqCst), 2);
}

#[test]
fn mutable_reference_input_test() {
    #[directed::stage]
    fn Source() -> String {
        String::from("hi")
    }

    #[directed::stage]
    fn Exclaim(value: &mut String) -> usize {
        value.push('!');
        value.len()
    }

    #[directed::stage]
    fn Sink(len: usize) {
        assert_eq!(len, 3);
    }

    let mut registry = Registry::new();
    let source = registry.register::<Source>();
    let exclaim = registry.register::<Exclaim>();
    let sink = registry.register::<Sink>();

    let graph = directed::graph! {
        nodes: [source, exclaim, sink],
        connections: {
            source: out => exclaim: value,
            exclaim: out => sink: len,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn generic_stage_test() {
    #[directed::stage]
    fn Identity<T>(value: T) -> T {
        value
    }

    #[directed::stage]
    fn MakeString() -> String {
        String::from("hello")
    }

    #[directed::stage]
    fn StringSink(value: String) {
        assert_eq!(value, "hello");
    }

    let mut registry = Registry::new();
    let source = registry.register::<MakeString>();
    let identity = registry.register::<Identity<String>>();
    let sink = registry.register::<StringSink>();

    let graph = directed::graph! {
        nodes: [source, identity, sink],
        connections: {
            source: out => identity: value,
            identity: out => sink: value,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn generic_stage_distinct_instantiations() {
    #[directed::stage]
    fn Identity<T>(value: T) -> T {
        value
    }

    #[directed::stage]
    fn IntSource() -> i32 {
        7
    }

    #[directed::stage]
    fn StrSource() -> String {
        String::from("x")
    }

    #[directed::stage]
    fn IntSink(value: i32) {
        assert_eq!(value, 7);
    }

    #[directed::stage]
    fn StrSink(value: String) {
        assert_eq!(value, "x");
    }

    let mut registry = Registry::new();
    let int_source = registry.register::<IntSource>();
    let str_source = registry.register::<StrSource>();
    let int_id = registry.register::<Identity<i32>>();
    let str_id = registry.register::<Identity<String>>();
    let int_sink = registry.register::<IntSink>();
    let str_sink = registry.register::<StrSink>();

    // The two `Identity` instantiations must have distinct signatures.
    let int_signature = <Identity<i32> as directed::Stage>::signature();
    let str_signature = <Identity<String> as directed::Stage>::signature();
    assert_ne!(
        int_signature.inputs[0].ops.type_id,
        str_signature.inputs[0].ops.type_id
    );

    let graph = directed::graph! {
        nodes: [int_source, str_source, int_id, str_id, int_sink, str_sink],
        connections: {
            int_source: out => int_id: value,
            int_id: out => int_sink: value,
            str_source: out => str_id: value,
            str_id: out => str_sink: value,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
}

#[test]
fn generic_stage_with_cache() {
    #[directed::stage(lazy, cache_last)]
    fn Identity<T>(value: T) -> T {
        value
    }

    #[directed::stage]
    fn Source() -> i32 {
        21
    }

    #[directed::stage]
    fn Sink(value: i32) {
        assert_eq!(value, 21);
    }

    let mut registry = Registry::new();
    let source = registry.register::<Source>();
    let identity = registry.register::<Identity<i32>>();
    let sink = registry.register::<Sink>();

    let graph = directed::graph! {
        nodes: [source, identity, sink],
        connections: {
            source: out => identity: value,
            identity: out => sink: value,
        }
    }
    .unwrap();

    graph.execute(&registry).unwrap();
    graph.execute(&registry).unwrap();
}

#[test]
fn distinct_generic_stages_same_type_arg() {
    #[directed::stage]
    fn Passthrough<T>(value: T) -> T {
        value
    }

    #[directed::stage]
    fn WrapVec<T>(value: T) -> Vec<T> {
        vec![value]
    }

    let passthrough = <Passthrough<i32> as directed::Stage>::signature();
    let wrap = <WrapVec<i32> as directed::Stage>::signature();

    assert_eq!(passthrough.stage, "Passthrough");
    assert_eq!(wrap.stage, "WrapVec");
    // Different generic stages with the same type argument must not share a
    // signature (regression: interning keyed only on the type arguments).
    assert!(!std::ptr::eq(passthrough, wrap));
    assert_ne!(
        passthrough.outputs[0].ops.type_id,
        wrap.outputs[0].ops.type_id
    );
}
