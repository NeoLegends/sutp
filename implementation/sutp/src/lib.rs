#![allow(dead_code)]

// Macros need to lexically come before the rest to be usable
#[macro_use]
mod macros;

mod accept;
mod chunk;
mod connect;
mod driver;
mod listener;
mod segment;
mod sparse_buf;
mod stream;

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

/// Some useful extensions to `Result`.
trait ResultExt<T, E> {
    /// Allows mutable transformation on the value, without requiring the value
    /// to be returned.
    ///
    /// This is a shorthand for:
    ///
    /// ```rust
    /// # let result: Result<usize, ()> = Ok(5);
    /// # fn do_something(v: &mut usize) -> Result<usize, ()> { Ok(*v) }
    ///
    /// result.and_then(|mut v| {
    ///     do_something(&mut v)?;
    ///     Ok(v)
    /// });
    /// ```
    fn inspect_mut(self, f: impl FnOnce(&mut T) -> Result<(), E>) -> Self;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn inspect_mut(self, f: impl FnOnce(&mut T) -> Result<(), E>) -> Self {
        self.and_then(|mut val| {
            f(&mut val)?;
            Ok(val)
        })
    }
}
