use core::mem::MaybeUninit;
use tracing::{event, Id, Level};
use tracing_core::{Collect, Dispatch};
use tracing_serde_wire::Packet;
use tracing_serde_subscriber::TSCollector;

fn main() {
    let disp = Dispatch::new(TSCollector::new());
    tracing::dispatch::set_global_default(disp).unwrap();

    let coll = TSCollector::take_receiver().unwrap();

    let span = tracing::span!(Level::TRACE, "outer_span");
    let _span = span.enter();
    do_thing::doit();

    // You could do this in another thread...
    while let Ok(msg) = coll.try_recv() {
        println!("{}", serde_json::to_string(&msg).unwrap());
    }
    println!("Done.");

    // println!("===========================");

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
