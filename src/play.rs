static PC: PlayCollector = PlayCollector;
static CTR: AtomicU32 = AtomicU32::new(1);

struct PlayCollector;

fn next_span_id() -> Id {
    loop {
        let next = CTR.fetch_add(1, Ordering::SeqCst);
        if let Some(nzu64) = NonZeroU64::new(next as u64) {
            return Id::from_non_zero_u64(nzu64);
        }
    }
}

fn line() {
    println!("==================================================");
}

impl Collect for PlayCollector {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        line();
        println!(
            "ENABLED: {}",
            serde_json::to_string(&metadata.as_serde()).unwrap()
        );

        // If we support filtering at some point, this should be changed.
        true
    }

    fn new_span(&self, span: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
        line();
        println!(
            "NEW SPAN: {}",
            serde_json::to_string(&span.as_serde()).unwrap()
        );
        next_span_id()
    }

    fn record(&self, span: &tracing_core::span::Id, values: &tracing_core::span::Record<'_>) {
        line();
        println!(
            "RECORD id: {} values: {}",
            serde_json::to_string(&span.as_serde()).unwrap(),
            serde_json::to_string(&values.as_serde()).unwrap(),
        );
    }

    fn record_follows_from(&self, span: &tracing_core::span::Id, follows: &tracing_core::span::Id) {
        line();
        println!(
            "RECFOLFRM id: {} values: {}",
            serde_json::to_string(&span.as_serde()).unwrap(),
            serde_json::to_string(&follows.as_serde()).unwrap(),
        );
    }

    fn event(&self, event: &tracing::Event<'_>) {
        line();
        println!(
            "EVENT {}",
            serde_json::to_string(&event.as_serde()).unwrap()
        );
    }

    fn enter(&self, span: &tracing_core::span::Id) {
        line();
        println!("ENTER {}", serde_json::to_string(&span.as_serde()).unwrap());
    }

    fn exit(&self, span: &tracing_core::span::Id) {
        line();
        println!("EXIT {}", serde_json::to_string(&span.as_serde()).unwrap());
    }

    fn current_span(&self) -> Current {
        Current::unknown()
    }
}
