#![no_std]

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    num::NonZeroU64,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use bbqueue_sync::{BBBuffer, Consumer, Producer};
use core::mem::MaybeUninit;
use groundhog::RollingTimer;
use rzcobs::{Write, self};
use tracing_core::{span::{Current, Id}, Collect};
use tracing_serde::AsSerde;
use tracing_serde_wire::Packet;
use tracing_serde_wire::TracingWire;
use critical_section::with;

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


    fn encode(&self, msg: TracingWire<'_>) {
        let _ = with(|_cs| -> Result<(), ()> {
            // Attempt to get the buffer. Fails if it was not initialized
            let prod = self.get_prod()?;

            // Attemt to get a write space. Fails if we don't have enough space
            let mut wgr = prod.grant_exact(MAX_SINGLE).map_err(drop)?;

            // SAFETY: We are in a critical section, and this region was initialized
            // in the init function.
            let scratch = unsafe { (*self.scratch.get()).assume_init_mut() };
            let timer = TIMER::default();
            let msg = Packet {
                message: msg,
                tick: timer.get_ticks() as u64,
            };

            // Serialize to postcard, NOT COBS encoded
            let used = postcard::to_slice(&msg, scratch.as_mut_slice())
                .map_err(drop)?;

            // Encode to COBS for framing
            let mut enc = rzcobs::Encoder::new(RWriter {
                offset: 0,
                buf: &mut wgr,
            });
            used.iter().try_for_each(|b| enc.write(*b))?;
            enc.end()?;
            enc.writer().push_one(0)?;
            let offset = enc.writer().offset;
            wgr.commit(offset);

            Ok(())
        });
    }
}

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

impl<TIMER, const TTL_SIZE: usize, const MAX_SINGLE: usize> Collect
    for BBQCollector<TIMER, TTL_SIZE, MAX_SINGLE>
where
    TIMER: RollingTimer<Tick = u32> + Default + 'static,
{
    fn enabled(&self, _metadata: &tracing_core::Metadata<'_>) -> bool {
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

    fn event(&self, event: &tracing_core::Event<'_>) {
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
