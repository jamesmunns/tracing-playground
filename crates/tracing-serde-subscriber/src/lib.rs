use once_cell::sync::Lazy;
use tracing_core::span::Id;
use tracing_core::{Collect, span::Current};
use tracing_serde_wire::{Packet, TracingWire};
use std::num::NonZeroU64;
use std::sync::atomic::Ordering;
use std::time::Instant;
use crossbeam_channel::{unbounded, Sender, Receiver};
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use tracing_serde::AsSerde;

struct Container {
    send: Sender<Packet<'static>>,
    recv: Mutex<Option<Receiver<Packet<'static>>>>,
    // TODO: Thread local counter?
    ctr: AtomicU64,
    start: Instant,
}

static CONTAINER: Lazy<Container> = Lazy::new(|| {
    // TODO: Should we bound the collector queue? At the moment this will grow unbounded.
    // We could do (at least) one of these three strategies:
    //
    // 1. Collector grows unbounded (fast + no loss - but could exhaust memory) <-- Current choice
    // 2. Collector blocks when the queue is full (acts as backpressure - but slows app speed)
    // 3. Collector doesn't block, but counts "missed" traces (no backpressure - but can drop traces)
    //
    // Not sure if there is a "one size fits all choice". Picking the easiest one for now.
    let (sender, receiver) = unbounded();
    Container {
        send: sender,
        recv: Mutex::new(Some(receiver)),
        ctr: AtomicU64::new(1),
        start: Instant::now(),
    }
});

pub struct TSCollector {
    _me: (),
}

impl TSCollector {
    pub fn new() -> Self {
        TSCollector { _me: () }
    }

    /// Give back the consumer exactly once. This is done to avoid multiple sinks
    /// getting partial data
    pub fn take_receiver() -> Option<Receiver<Packet<'static>>> {
        // TODO: I *could* wrap the receiver in a newtype, and impl Drop on it to
        // "restore" the Receiver to the container. But I don't think anyone would
        // actually want to do that? Open an issue if that's something you want.
        CONTAINER.recv
            // If the receiver is already locked - someone is already trying to take
            // it. Don't wait around to try and get the lock
            .try_lock()
            // Convert the lock Result type directly into an Option type, to make
            // method chaining easier (and we don't care *why* the lock failed,
            // to be quite honest)
            .ok()
            // If we received the lock, try to take the Receiver. If it has already
            // been taken, this will return a None. Otherwise it will return a
            // Some(Receiver).
            .and_then(|mut r| r.take())
    }
}

impl TSCollector {
    fn get_next_id(&self) -> tracing_core::span::Id {
        loop {
            let next = CONTAINER.ctr.fetch_add(1, Ordering::SeqCst);
            if let Some(nzu64) = NonZeroU64::new(next as u64) {
                return Id::from_non_zero_u64(nzu64);
            }
        }
    }
}

// NOTE: All `send()`s currently ignore any return values. This would only occur
// if the receiver side has been dropped. In that case, just move on without
// causing a fuss.
impl Collect for TSCollector
{
    fn enabled(&self, _metadata: &tracing_core::Metadata<'_>) -> bool {
        // Note: nothing to log here, this is a `query` whether the trace is active
        // TODO: always enabled for now.
        true
    }

    fn new_span(&self, span: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
        CONTAINER.send.send(Packet {
            message: TracingWire::NewSpan(span.as_serde().to_owned()),
            // NOTE: will give inaccurate data if the program has run for more than 584942 years.
            tick: CONTAINER.start.elapsed().as_micros() as u64,
        }).ok();
        self.get_next_id()
    }

    fn record(&self, span: &tracing_core::span::Id, values: &tracing_core::span::Record<'_>) {
        CONTAINER.send.send(Packet {
            message: TracingWire::Record {
                span: span.as_serde(),
                values: values.as_serde().to_owned(),
            },
            // NOTE: will give inaccurate data if the program has run for more than 584942 years.
            tick: CONTAINER.start.elapsed().as_micros() as u64,
        }).ok();
    }

    fn record_follows_from(&self, span: &tracing_core::span::Id, follows: &tracing_core::span::Id) {
        CONTAINER.send.send(Packet {
            message: TracingWire::RecordFollowsFrom {
                span: span.as_serde(),
                follows: follows.as_serde().to_owned(),
            },
            // NOTE: will give inaccurate data if the program has run for more than 584942 years.
            tick: CONTAINER.start.elapsed().as_micros() as u64,
        }).ok();
    }

    fn event(&self, event: &tracing_core::Event<'_>) {
        CONTAINER.send.send(Packet {
            message: TracingWire::Event(event.as_serde().to_owned()),
            // NOTE: will give inaccurate data if the program has run for more than 584942 years.
            tick: CONTAINER.start.elapsed().as_micros() as u64,
        }).ok();
    }

    fn enter(&self, span: &tracing_core::span::Id) {
        CONTAINER.send.send(Packet {
            message: TracingWire::Enter(span.as_serde()),
            // NOTE: will give inaccurate data if the program has run for more than 584942 years.
            tick: CONTAINER.start.elapsed().as_micros() as u64,
        }).ok();
        // self.encode(TracingWire::Enter(span.as_serde()))
    }

    fn exit(&self, span: &tracing_core::span::Id) {
        CONTAINER.send.send(Packet {
            message: TracingWire::Exit(span.as_serde()),
            // NOTE: will give inaccurate data if the program has run for more than 584942 years.
            tick: CONTAINER.start.elapsed().as_micros() as u64,
        }).ok();
        // self.encode(TracingWire::Exit(span.as_serde()))
    }

    fn current_span(&self) -> tracing_core::span::Current {
        Current::unknown()
    }
}
