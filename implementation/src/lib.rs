extern crate byteorder;
extern crate bytes;
extern crate flate2;
extern crate futures;
#[macro_use] extern crate log;
extern crate tokio;

// Macros need to lexically come before the rest
#[macro_use] mod macros;

mod chunk;
mod codec;
mod protocol;
mod segment;

pub use protocol::SutpStream;
