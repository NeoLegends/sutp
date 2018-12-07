use futures::{
    prelude::*,
    sync::mpsc,
    try_ready,
};
use rand;
use std::{
    io::{self, Error, ErrorKind, Read, Write},
    net::SocketAddr,
    num::Wrapping,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::udp::UdpSocket,
};

use crate::connect::Connect;
use crate::segment::Segment;

/// A full-duplex SUTP stream.
#[derive(Debug)]
pub struct SutpStream {
    local_sq_no: Wrapping<u32>,
    recv: mpsc::Receiver<Result<Segment, io::Error>>,
    remote_sq_no: Wrapping<u32>,
    send_socket: UdpSocket,
    state: StreamState,
}

/// States of the protocol automaton after the connection has been established.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
enum StreamState {
    /// The connection is open and full-duplex transport is possible.
    Open,

    /// A FIN chunk has been sent and the sending side has been closed.
    FinSent,

    /// A FIN chunk was received and the receiving side has been closed.
    ///
    /// Once the socket is in this state, already-buffered and yet-outstanding
    /// data can still be read, but anything after that will cause an error.
    FinRecvd,

    /// The connection is closed and no further data can be sent.
    Closed,
}

impl SutpStream {
    /// Creates an SUTP connection to the given address.
    ///
    /// When the returned future completes, the stream has been established
    /// and can be used to transmit data.
    pub fn connect(_addr: &SocketAddr) -> Connect {
        unimplemented!()
    }

    pub(crate) fn from_accept(
        recv: mpsc::Receiver<Result<Segment, Error>>,
        sock: UdpSocket,
        local_sq_no: Wrapping<u32>,
        remote_sq_no: Wrapping<u32>,
    ) -> Self {
        Self {
            local_sq_no,
            recv,
            remote_sq_no,
            send_socket: sock,
            state: StreamState::Open,
        }
    }
}

impl SutpStream {
    fn assert_can_write(&self) -> io::Result<()> {
        match self.state {
            StreamState::Open | StreamState::FinRecvd => Ok(()),
            _ => Err(ErrorKind::NotConnected.into()),
        }
    }

    fn try_read(&mut self, _buf: &mut [u8]) -> Poll<usize, io::Error> {
        match self.state {
            StreamState::Open => {},
            StreamState::FinRecvd => unimplemented!(),
            _ => return Err(ErrorKind::NotConnected.into()),
        }

        unimplemented!()
    }

    fn try_write(&mut self, _buf: &[u8]) -> Poll<usize, io::Error> {
        self.assert_can_write()?;

        unimplemented!()
    }

    fn try_flush(&mut self) -> Poll<(), io::Error> {
        self.assert_can_write()?;

        unimplemented!()
    }
}

impl Read for SutpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        try_would_block!(self.try_read(buf))
    }
}

impl AsyncRead for SutpStream {}

impl Write for SutpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        try_would_block!(self.try_write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        try_would_block!(self.try_flush())
    }
}

impl AsyncWrite for SutpStream {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        try_ready!(self.try_flush());

        unimplemented!()
    }
}
