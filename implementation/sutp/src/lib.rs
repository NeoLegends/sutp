//! An implementation of the [2018 COMSYS Software Exercise Class](https://laboratory.comsys.rwth-aachen.de/sutp/data-format)
//! transport protocol.
//!
//! This implementation uses asynchronous I/O using the [tokio](https://tokio.rs)
//! and [futures](https://github.com/rust-lang-nursery/futures-rs) stack.
//!
//! New connection can either be made (client-side) using `SutpStream::connect`
//! or via setting up an `SutpListener` and accepting new connections on it.
//!
//! See [SutpStream](struct.SutpStream.html) or [SutpListener](struct.SutpListener.html)
//! for more detail.

mod accept;
mod chunk;
mod connect;
mod driver;
mod listener;
mod segment;
mod stream;
mod window;

pub use crate::{
    accept::Accept,
    connect::Connect,
    listener::{Incoming, SutpListener},
    stream::SutpStream,
};

use std::{time::Duration, u16};

/// The timeout for setting up a new connection.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// The error message when the driver has shutdown unexpectedly in the background.
const DRIVER_AWAY: &str = "driver has gone away";

/// The error message when a channel doesn't have a segment, but actually should.
const MISSING_SEGMENT: &str = "missing segment";

/// The error message for when a receiver unexpectedly fails.
///
/// This cannot happen usually (as receivers return Ok(Async::Ready(None)) in case
/// the senders were dropped), so this message is primarily used for .expect().
const RECEIVER_ERROR: &str = "mpsc::Receiver error";

/// The timeout for receiving the response to a segment.
const RESPONSE_SEGMENT_TIMEOUT: Duration = Duration::from_secs(2);

/// Max size of a UDP datagram.
const UDP_DGRAM_SIZE: usize = u16::MAX as usize;
