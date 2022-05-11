use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    num::NonZeroU64,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use bbqueue_sync::{BBBuffer, Consumer, GrantW, Producer};
use core::mem::MaybeUninit;
use groundhog::{std_timer::Timer, RollingTimer};
use rzcobs::{Write, self};
use tracing::{event, Id, Level};
use tracing_core::{span::Current, Collect, Dispatch};
use tracing_serde::AsSerde;
use tracing_serde_wire::Packet;
use tracing_serde_wire::TracingWire;

// fn main() {
//     let msg: &[u8] = &[
//         0, 10, 111, 117, 116, 101, 114, 95, 115, 112, 97, 110, 18, 116, 114, 97, 99, 105, 110, 103,
//         95, 112, 108, 97, 121, 103, 114, 111, 117, 110, 100, 0, 1, 18, 116, 114, 97, 99, 105, 110,
//         103, 95, 112, 108, 97, 121, 103, 114, 111, 117, 110, 100, 1, 11, 115, 114, 99, 47, 109, 97,
//         105, 110, 46, 114, 115, 1, 23, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
//     ];
//     let enc = rzcobs::encode(msg);
//     let dec = rzcobs::decode(&enc).unwrap();
//     assert_eq!(msg, &dec);
// }

fn main() {
    let cons = BQ.init().unwrap();
    let disp = Dispatch::from_static(&BQ);
    tracing::dispatch::set_global_default(disp).unwrap();

    let span = tracing::span!(Level::TRACE, "outer_span");
    let _span = span.enter();
    do_thing::doit();

    // println!("===========================");

    let mut data = cons.read().unwrap().to_vec();

    let mut ct = 0;

    while !data.is_empty() {
        let pos = match data.iter().position(|b| *b == 0) {
            Some(p) => p,
            None => {
                println!("What?");
                break;
            }
        };

        let later = data.split_off(pos + 1);
        // println!("{:?}", data);
        assert_eq!(Some(0), data.pop());
        let decoded = rzcobs::decode(&data[..data.len()]).unwrap();
        match postcard::from_bytes::<Packet<'_>>(&decoded) {
            Ok(p) => {}, // println!("{:?}", p),
            Err(e) => {
                println!(">>>{}<<<", ct);
                println!("!!!{:?}!!!", &data);
                println!("{:?}", &decoded);
                panic!("{:?}", e);
            }
        };
        data = later;
        ct += 1;
    }
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

static BQ: BBQCollector<Timer<1_000_000>, 16384, 512> = BBQCollector::new();

pub struct BBQCollector<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize> {
    initialized: AtomicBool,
    bbq: BBBuffer<TTL_SIZE>,
    prod: UnsafeCell<MaybeUninit<Producer<'static, TTL_SIZE>>>,
    ctr: AtomicU32,
    scratch: UnsafeCell<MaybeUninit<[u8; MAX_SINGLE]>>,
    _pdt: PhantomData<TIMER>,
}

unsafe impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize> Sync
    for BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
where
    TIMER: RollingTimer<Tick = u32> + Default + 'static,
{
}

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
        unsafe { Ok(&*self.prod.get().cast()) }
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
        unsafe { (*self.scratch.get()).assume_init_mut() }
    }

    fn encode(&self, msg: TracingWire<'_>) {
        // println!("{:?}", msg);
        // Attempt to get the buffer. Fails if it was not initialized
        let prod = if let Ok(prod) = self.get_prod() {
            prod
        } else {
            return;
        };

        // Attemt to get a write space. Fails if we don't have enough space
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

        // Serialize to postcard, NOT COBS encoded
        let used = match postcard::to_slice(&msg, scratch.as_mut_slice()) {
            Ok(used) => used,
            Err(_) => {
                // TODO: attempt to recover
                return;
            }
        };

        // println!("{:?}", used);

        #[derive(Debug)]
        struct RWriter<'a> {
            offset: usize,
            buf: &'a mut [u8],
        }

        impl<'a> RWriter<'a> {
            #[inline(always)]
            fn push_one(&mut self, byte: u8) -> Result<(), ()> {
                *self.buf.get_mut(self.offset).ok_or(())? = byte;
                self.offset += 1;
                Ok(())
            }
        }

        impl<'a> Write for RWriter<'a> {
            type Error = ();

            #[inline(always)]
            fn write(&mut self, byte: u8) -> Result<(), Self::Error> {
                self.push_one(byte)
            }
        }

        let venc = rzcobs::encode(used);
        let vdec = rzcobs::decode(&venc).unwrap();
        // assert_eq!(used, &vdec);

        // Encode to COBS for framing
        let mut enc = rzcobs::Encoder::new(RWriter {
            offset: 0,
            buf: &mut wgr,
        });
        let res = used.iter().try_for_each(|b| enc.write(*b));
        if res.is_ok() {
            if enc.end().is_ok() {
                if enc.writer().push_one(0).is_ok() {
                    let offset = enc.writer().offset;
                    // println!("--{:?}", &wgr[..offset]);
                    // assert_eq!(&venc, &wgr[..offset-1]);
                    wgr.commit(offset);
                } else {
                    // TODO recover
                }
            } else {
                // TODO recover
            }
        } else {
            // TODO recover
        }
    }
}

impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize> Collect
    for BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
where
    TIMER: RollingTimer<Tick = u32> + Default + 'static,
{
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        // Note: nothing to log here, this is a `query` whether the trace is active
        // TODO: always enabled for now.
        true
    }

    fn new_span(&self, span: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        self.encode(TracingWire::NewSpan(span.as_serde()));
        self.get_next_id()
    }

    fn record(&self, span: &tracing_core::span::Id, values: &tracing_core::span::Record<'_>) {
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(values)).unwrap());
        self.encode(TracingWire::Record {
            span: span.as_serde(),
            values: values.as_serde(),
        });
    }

    fn record_follows_from(&self, span: &tracing_core::span::Id, follows: &tracing_core::span::Id) {
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(follows)).unwrap());
        self.encode(TracingWire::RecordFollowsFrom {
            span: span.as_serde(),
            follows: follows.as_serde(),
        });
    }

    fn event(&self, event: &tracing::Event<'_>) {
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(event)).unwrap());
        self.encode(TracingWire::Event(event.as_serde()));
    }

    fn enter(&self, span: &tracing_core::span::Id) {
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        self.encode(TracingWire::Enter(span.as_serde()))
    }

    fn exit(&self, span: &tracing_core::span::Id) {
        // println!("'{}'", serde_json::to_string(&tracing_serde::AsSerde::as_serde(span)).unwrap());
        self.encode(TracingWire::Exit(span.as_serde()))
    }

    fn current_span(&self) -> tracing_core::span::Current {
        Current::unknown()
    }
}
