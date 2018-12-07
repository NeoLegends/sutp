use futures::{
    prelude::*,
    sync::mpsc,
};
use rand;
use std::{
    io::{self, ErrorKind, Read, Write},
    net::SocketAddr,
    num::Wrapping,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::udp::{UdpFramed, UdpSocket},
};

use crate::codec::SutpCodec;
use crate::segment::Segment;

/// A full-duplex SUTP stream.
#[derive(Debug)]
pub struct SutpStream {
    local_sq_no: Wrapping<u32>,
    recv: mpsc::Receiver<Result<Segment, io::Error>>,
    remote_sq_no: Wrapping<u32>,
    send_socket: UdpFramed<SutpCodec>,
    state: State,
}

/// An SUTP stream during inital connection.
#[derive(Debug)]
pub struct Connect {

}

/// States of the protocol automaton.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum State {
    /// The connection is in the process of being opened and there were no
    /// segments sent so far.
    ///
    /// This is the start state for new connections made directly through the
    /// stream by `connect`ing it.
    BeforeOpen,

    /// The connection has sent the first SYN->, but is still waiting for
    /// <-SYN acking the first one.
    SynSent,

    /// The stream has received the first SYN-> and is in the progress of
    /// ACKing that.
    ///
    /// This is the start state for new connections coming from the listener.
    SynRcvd,

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

impl Future for Connect {
    type Item = SutpStream;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        unimplemented!()
    }
}

impl SutpStream {
    /// Attempts to create a connection to the given remote.
    ///
    /// When this function returns, the connection has not yet been
    /// established. This will be done on the first usage of the socket.
    pub fn connect(_addr: &SocketAddr) -> io::Result<Self> {
        unimplemented!()
    }

    /// Constructs an SUTP stream.
    pub(crate) fn from_listener(
        sock: UdpSocket,
        recv: mpsc::Receiver<Result<Segment, io::Error>>,
        initial_state: State,
    ) -> Connect {
        let _l = Self {
            local_sq_no: Wrapping(rand::random()),
            recv: recv,
            remote_sq_no: Wrapping(0),
            send_socket: UdpFramed::new(sock, SutpCodec),
            state: initial_state,
        };

        unimplemented!()
    }
}

impl SutpStream {
    fn assert_can_write(&self) -> io::Result<()> {
        match self.state {
            State::Open | State::FinRecvd => Ok(()),
            _ => Err(ErrorKind::NotConnected.into()),
        }
    }

    fn try_read(&mut self, _buf: &mut [u8]) -> Poll<usize, io::Error> {
        match self.state {
            State::Open => {},
            State::FinRecvd => unimplemented!(),
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
        unimplemented!()
    }
}
