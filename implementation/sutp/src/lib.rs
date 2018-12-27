// Macros need to lexically come before the rest to be usable
#[macro_use]
mod macros;

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

/// Max size of a UDP datagram.
const UDP_DGRAM_SIZE: usize = u16::MAX as usize;

/// The timeout for setting up a new connection.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// The timeout for receiving the response to a segment.
const RESPONSE_SEGMENT_TIMEOUT: Duration = Duration::from_secs(2);
