use futures::sync::mpsc;
use rand;
use std::{
    io,
    net::SocketAddr,
    num::Wrapping,
};
use tokio::net::udp::{UdpFramed, UdpSocket};

use codec::SutpCodec;
use segment::Segment;

/// A full-duplex SUTP stream.
#[derive(Debug)]
pub struct SutpStream {
    local_sq_no: Wrapping<u32>,
    recv: mpsc::Receiver<Result<Segment, io::Error>>,
    remote_sq_no: Wrapping<u32>,
    send_socket: UdpFramed<SutpCodec>,
    state: State,
}

/// The connection state.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
enum State {
    Null,
}

impl SutpStream {
    /// Attempts to create a connection to the given remote.
    ///
    /// When this function returns, the connection has not yet been
    /// established. This will be done on the first usage of the socket
    pub fn connect(addr: &SocketAddr) -> io::Result<Self> {
        unimplemented!()
    }

    /// Constructs an SUTP stream.
    pub(crate) fn from_listener(
        sock: UdpSocket,
        recv: mpsc::Receiver<Result<Segment, io::Error>>,
    ) -> Self {
        Self {
            local_sq_no: Wrapping(rand::random()),
            recv: recv,
            remote_sq_no: Wrapping(0),
            send_socket: UdpFramed::new(sock, SutpCodec),
            state: State::Null,
        }
    }
}
