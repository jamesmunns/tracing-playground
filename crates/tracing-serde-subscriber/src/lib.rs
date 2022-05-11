struct TSCollector {

}

impl TSCollector {
    fn get_next_id(&self) -> tracing_core::span::Id {
        todo!()
    }
}


impl Collect for TSCollector
{
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        // Note: nothing to log here, this is a `query` whether the trace is active
        // TODO: always enabled for now.
        true
    }

    fn new_span(&self, span: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // self.encode(TracingWire::NewSpan(span.as_serde()));
        self.get_next_id()
    }

    fn record(&self, span: &tracing_core::span::Id, values: &tracing_core::span::Record<'_>) {
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(values)).unwrap());
        // self.encode(TracingWire::Record {
        //     span: span.as_serde(),
        //     values: values.as_serde(),
        // });
    }

    fn record_follows_from(&self, span: &tracing_core::span::Id, follows: &tracing_core::span::Id) {
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(follows)).unwrap());
        // self.encode(TracingWire::RecordFollowsFrom {
        //     span: span.as_serde(),
        //     follows: follows.as_serde(),
        // });
    }

    fn event(&self, event: &tracing::Event<'_>) {
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(event)).unwrap());
        // self.encode(TracingWire::Event(event.as_serde()));
    }

    fn enter(&self, span: &tracing_core::span::Id) {
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // self.encode(TracingWire::Enter(span.as_serde()))
    }

    fn exit(&self, span: &tracing_core::span::Id) {
        // // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // self.encode(TracingWire::Exit(span.as_serde()))
    }

    fn current_span(&self) -> tracing_core::span::Current {
        Current::unknown()
    }
}
