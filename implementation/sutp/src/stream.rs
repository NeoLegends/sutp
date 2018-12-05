use bytes::BytesMut;
use futures::prelude::*;
use rand;
use std::{
    io,
    net::SocketAddr,
    num::Wrapping,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::udp::{UdpFramed, UdpSocket},
};

use codec::SutpCodec;
use segment::Segment;

/// A full-duplex SUTP stream.
#[derive(Debug)]
pub struct SutpStream {
    local_sq_no: Wrapping<u32>,
    remote_sq_no: Wrapping<u32>,
    socket: UdpFramed<SutpCodec>,
    state: State,
}

#[derive(Debug)]
pub struct Connect {

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
        let socket = {
            let sock = UdpSocket::bind(&addr)?;
            sock.connect(&addr)?;
            sock
        };

        Ok(Self {
            local_sq_no: Wrapping(rand::random()),
            remote_sq_no: Wrapping(0),
            socket: UdpFramed::new(socket, SutpCodec),
            state: State::Null,
        })
    }

    /// Constructs an SUTP stream, connected to the given socket using the
    /// given buffer as read buffer containing the first segment.
    pub(crate) fn from_listener(
        socket: UdpSocket,
        recv_buf: BytesMut,
    ) -> io::Result<Self> {
        Ok(Self {
            local_sq_no: Wrapping(rand::random()),
            remote_sq_no: Wrapping(0),
            socket: UdpFramed::new(socket, SutpCodec),
            state: State::Null,
        })
    }
}
