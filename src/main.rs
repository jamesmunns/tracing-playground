use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    num::NonZeroU64,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use bbqueue_sync::{Producer, GrantW, BBBuffer, Consumer};
use rzcobs::Write;
use serde::{Serialize, Deserialize};
use core::mem::MaybeUninit;
use groundhog::{std_timer::Timer, RollingTimer};
use tracing::{event, Id, Level};
use tracing_core::{span::Current, Collect, Dispatch};
use tracing_serde::AsSerde;

static PC: PlayCollector = PlayCollector;
static CTR: AtomicU32 = AtomicU32::new(1);

static BQ: BBQCollector<Timer<1_000_000>, 16384, 512> = BBQCollector::new();

pub enum TWOther {
    MessageDiscarded,
    DeviceInfo {
        clock_id: u32,
        ticks_per_sec: u32,
        device_id: [u8; 16],
    },
}

#[derive(Serialize)]
pub struct Packet<'a> {
    message: TracingWire<'a>,
    tick: u64,
}

#[derive(Serialize)]
pub enum TracingWire<'a> {
    Enabled(tracing_serde::SerializeMetadata<'a>),
    NewSpan(tracing_serde::SerializeAttributes<'a>),
    Record {
        values: tracing_serde::SerializeRecord<'a>,
        span: tracing_serde::SerializeId<'a>,
    },
    RecordFollowsFrom {
        follows: tracing_serde::SerializeId<'a>,
        span: tracing_serde::SerializeId<'a>,
    },
    Event(tracing_serde::SerializeEvent<'a>),
    Enter(tracing_serde::SerializeId<'a>),
    Exit(tracing_serde::SerializeId<'a>),
    Other,
}

pub struct BBQCollector<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize> {
    initialized: AtomicBool,
    bbq: BBBuffer<TTL_SIZE>,
    prod: UnsafeCell<MaybeUninit<Producer<'static, TTL_SIZE>>>,
    ctr: AtomicU32,
    scratch: UnsafeCell<MaybeUninit<[u8; MAX_SINGLE]>>,
    _pdt: PhantomData<TIMER>,
}

unsafe impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize>
    Sync for BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
where
    TIMER: RollingTimer<Tick = u32> + Default + 'static {}

impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize>
    BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
{
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            bbq: BBBuffer::new(),
            prod: UnsafeCell::new(MaybeUninit::uninit()),
            ctr: AtomicU32::new(0),
            scratch: UnsafeCell::new(MaybeUninit::uninit()),
            _pdt: PhantomData,
        }
    }
}

impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize>
    BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
where
    TIMER: RollingTimer<Tick = u32> + Default + 'static,
{
    const INVALID_ID: NonZeroU64 = unsafe { NonZeroU64::new_unchecked(0xFFFF_FFFF_FFFF_FFFF) };

    pub fn init(&'static self) -> Result<Consumer<'static, TTL_SIZE>, ()> {
        if self.initialized.swap(true, Ordering::SeqCst) {
            // Already initialized!
            return Err(());
        }

        let (prod, cons) = self.bbq.try_split().map_err(drop)?;

        unsafe {
            self.prod.get().write(MaybeUninit::new(prod));
            self.scratch.get().write(MaybeUninit::zeroed());
        }

        self.ctr.store(1, Ordering::SeqCst);

        Ok(cons)
    }

    pub fn get_prod(&self) -> Result<&Producer<'static, TTL_SIZE>, ()> {
        if !self.initialized.load(Ordering::SeqCst) {
            return Err(());
        }
        unsafe {
            Ok(&*self.prod.get().cast())
        }
    }

    pub fn get_next_id(&self) -> Id {
        if self.initialized.load(Ordering::SeqCst) {
            loop {
                let next = self.ctr.fetch_add(1, Ordering::SeqCst);
                if let Some(nzu64) = NonZeroU64::new(next as u64) {
                    return Id::from_non_zero_u64(nzu64);
                }
            }
        } else {
            Id::from_non_zero_u64(Self::INVALID_ID)
        }
    }

    fn get_scratch<'a>(&self, _grant: &'_ GrantW<'_, TTL_SIZE>) -> &mut [u8; MAX_SINGLE] {
        // SAFETY: The presence of a write grant shows that we own an exclusive lock to the writer
        // side of the BBQ Producer, preventing re-entrancy problems.
        unsafe {
            (*self.scratch.get()).assume_init_mut()
        }
    }

    fn encode(&self, msg: TracingWire<'_>) {
        let prod = if let Ok(prod) = self.get_prod() {
            prod
        } else {
            return;
        };

        let mut wgr = if let Ok(wgr) = prod.grant_exact(MAX_SINGLE) {
            wgr
        } else {
            // TODO: attempt to notify that we've dropped a message
            return;
        };

        let scratch = self.get_scratch(&mut wgr);
        let timer = TIMER::default();
        let msg = Packet {
            message: msg,
            tick: timer.get_ticks() as u64,
        };

        let used = match postcard::to_slice(&msg, scratch.as_mut_slice()) {
            Ok(used) => used,
            Err(_) => {
                // TODO: attempt to recover
                return;
            }
        };

        struct RWriter<'a> {
            offset: usize,
            buf: &'a mut [u8],
        }

        impl<'a> Write for RWriter<'a> {
            type Error = ();

            fn write(&mut self, byte: u8) -> Result<(), Self::Error> {
                *self.buf.get_mut(self.offset).ok_or(())? = byte;
                self.offset += 1;
                Ok(())
            }
        }

        let mut enc = rzcobs::Encoder::new(RWriter { offset: 0, buf: &mut wgr });
        let res = used.iter().try_for_each(|b| enc.write(*b));
        if res.is_ok() {
            if enc.writer().write(0).is_ok() {
                let offset = enc.writer().offset;
                wgr.commit(offset);
            }
        }
    }
}

impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize>
    Collect for BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
where
    TIMER: RollingTimer<Tick = u32> + Default + 'static,
{
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        // TODO: always enabled for now.
        self.encode(TracingWire::Enabled(metadata.as_serde()));
        true
    }

    fn new_span(&self, span: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
        self.encode(TracingWire::NewSpan(span.as_serde()));
        self.get_next_id()
    }

    fn record(&self, span: &tracing_core::span::Id, values: &tracing_core::span::Record<'_>) {
        self.encode(TracingWire::Record {
            span: span.as_serde(),
            values: values.as_serde(),
        });
    }

    fn record_follows_from(&self, span: &tracing_core::span::Id, follows: &tracing_core::span::Id) {
        self.encode(TracingWire::RecordFollowsFrom {
            span: span.as_serde(),
            follows: follows.as_serde(),
        });
    }

    fn event(&self, event: &tracing::Event<'_>) {
        self.encode(TracingWire::Event(event.as_serde()));
    }

    fn enter(&self, span: &tracing_core::span::Id) {
        self.encode(TracingWire::Enter(span.as_serde()))
    }

    fn exit(&self, span: &tracing_core::span::Id) {
        self.encode(TracingWire::Exit(span.as_serde()))
    }

    fn current_span(&self) -> tracing_core::span::Current {
        Current::unknown()
    }
}

fn main() {
    let cons = BQ.init().unwrap();
    let disp = Dispatch::from_static(&BQ);
    tracing::dispatch::set_global_default(disp).unwrap();
    println!("Hello, world!");

    let span = tracing::span!(Level::TRACE, "outer_span");
    let _ = span.enter();
    do_thing::doit();

    let rgr = cons.read().unwrap();
    println!("{:?}", &rgr);
}

pub mod do_thing {
    use super::*;
    pub fn doit() {
        let span = tracing::span!(Level::TRACE, "my span");
        span.in_scope(|| {
            event!(Level::INFO, "something has happened!");
        });
    }
}

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
