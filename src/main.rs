#[macro_use]
extern crate tracing;

use crate::trace::tracing_init;

mod trace;

fn main() {
    tracing_init();
    info!("Hello, world!");
}
