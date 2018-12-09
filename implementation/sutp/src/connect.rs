//! Implements the future that builds up a new connection.

use crate::{
    ResultExt,
    CONNECTION_TIMEOUT,
    RESPONSE_SEGMENT_TIMEOUT,
    chunk::{Chunk, SUPPORTED_COMPRESSION_ALGS},
    driver::{Driver, NEW_CONN_QUEUE_SIZE},
    segment::{Segment, SegmentBuilder},
    stream::SutpStream,
};
use futures::{
    prelude::*,
    sync::{mpsc, oneshot},
    try_ready,
};
use lazy_static::lazy_static;
use rand;
use std::{
    io::{Error, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::Wrapping,
};
use tokio::{
    clock,
    net::UdpSocket,
    timer::Delay,
};

const POLLED_TWICE: &str = "cannot poll Connect twice";

lazy_static! {
    /// The address to create local sockets with.
    ///
    /// This, when used for binding sockets, auto-selects a random free port.
    static ref BIND_ADDR: SocketAddr =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
}

/// A future representing the connection of a new SUTP stream.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Connect {
    /// The remote address to connect to.
    addr: SocketAddr,

    /// Underlying connection primitives.
    ///
    /// To honor that futures in Rust are created "cold" and only do things when
    /// polled, we create the actual networking objects on the first poll instead
    /// of during construction of this future.
    ///
    /// This is when this turns `Some`.
    inner: Option<Inner>,
}

/// Connection primitives for a single socket.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
struct Inner {
    /// A channel receiving hard I/O errors.
    io_err: Option<oneshot::Receiver<Error>>,

    /// A buffer containing the initial segment to be sent in binary form.
    init_segment_buf: Vec<u8>,

    /// The local sequence number.
    local_seq_no: Wrapping<u32>,

    /// The channel of incoming segments.
    recv: Option<mpsc::Receiver<Result<Segment, Error>>>,

    /// The remote sequence number.
    remote_seq_no: Wrapping<u32>,

    /// The socket to send segments over.
    sock: Option<UdpSocket>,

    /// The internal automaton state.
    state: State,

    /// The timeout guarding the whole connection setup.
    ///
    /// When this elapses the future will resolve with an error.
    timeout: Delay,
}

/// The internal state of the connect future automaton.
#[derive(Debug)]
enum State {
    /// The connection future was created, and the initial segment needs to be sent.
    Start,

    /// The first segment has been sent and we're waiting for the response.
    WaitingForResponse(Delay),
}

impl Connect {
    /// Creates a new `Connect` future.
    pub(crate) fn new(addr: &SocketAddr) -> Self {
        Connect {
            addr: *addr,
            inner: None,
        }
    }
}

impl Future for Connect {
    type Item = SutpStream;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if self.inner.is_none() {
            self.inner = Some(Inner::new(&self.addr)?);
        }

        self.inner.as_mut().unwrap().poll()
    }
}

impl Inner {
    /// Creates a new connection to the given `addr`.
    ///
    /// # Panics
    ///
    /// Panics if the driver cannot be spawned to the default executor.
    pub fn new(addr: &SocketAddr) -> Result<Self, Error> {
        let (err_tx, err_rx) = oneshot::channel();
        let (sgmt_tx, sgmt_rx) = mpsc::channel(NEW_CONN_QUEUE_SIZE);

        // Spawn a driver for this connection
        let driver = Driver::from_connection(
            UdpSocket::bind(&BIND_ADDR)?,
            err_tx,
            addr,
            sgmt_tx,
        );
        tokio::spawn(driver);

        // Set up necessary state
        let conn_timeout = {
            let elapsed_at = clock::now() + CONNECTION_TIMEOUT;
            Delay::new(elapsed_at)
        };
        let seq_no = Wrapping(rand::random());
        let initial_sgmt_buf = {
            let comp_algs = SUPPORTED_COMPRESSION_ALGS.as_ref().into();
            SegmentBuilder::new()
                .seq_no(seq_no.0)
                .window_size(16 * 1024)
                .with_chunk(Chunk::Syn)
                .with_chunk(Chunk::CompressionNegotiation(comp_algs))
                .build()
                .to_vec()
        };
        let socket = UdpSocket::bind(&BIND_ADDR)
            .inspect_mut(|s| s.connect(addr))?;

        Ok(Self {
            io_err: Some(err_rx),
            init_segment_buf: initial_sgmt_buf,
            local_seq_no: seq_no,
            recv: Some(sgmt_rx),
            remote_seq_no: Wrapping(0),
            sock: Some(socket),
            state: State::Start,
            timeout: conn_timeout,
        })
    }

    /// Polls for any kind of connection error.
    ///
    /// # Panics
    ///
    /// Panics if the driver has shut down unexpectedly.
    pub fn poll_error(&mut self) -> Result<(), Error> {
        self.poll_timeout()?;
        self.poll_io_error()
    }

    /// Checks if there were IO errors in the driver.
    ///
    /// # Panics
    ///
    /// Panics if the driver has shut down unexpectedly.
    fn poll_io_error(&mut self) -> Result<(), Error> {
        match self.io_err.as_mut().expect(POLLED_TWICE).poll() {
            Ok(Async::Ready(e)) => Err(e),
            Ok(Async::NotReady) => Ok(()),
            Err(_) => panic!("driver has gone away"),
        }
    }

    /// Polls the underlying stream for a new segment and returns Async::NotReady
    /// if there's none.
    ///
    /// # Panics
    ///
    /// Panics if the receiver stream has ended. This shouldn't happen
    /// normally, though.
    fn poll_segment(&mut self) -> Poll<Result<Segment, Error>, Error> {
        let val = self.recv.as_mut()
            .expect(POLLED_TWICE)
            .poll()
            .expect("driver has gone away")
            .map(|maybe_segment| maybe_segment.expect("missing segment"));
        Ok(val)
    }

    /// Checks whether the timeout for the setup has elapsed.
    fn poll_timeout(&mut self) -> Result<(), Error> {
        match self.timeout.poll() {
            Ok(Async::Ready(_)) => Err(ErrorKind::TimedOut.into()),
            Ok(Async::NotReady) => Ok(()),
            Err(e) => Err(Error::new(ErrorKind::Other, e)),
        }
    }
}

impl Future for Inner {
    type Item = SutpStream;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        // This implementation ping-pongs itself between the two possible states
        // of the future. In the first one, we send the SYN segment, and in the
        // second one we wait for and process the response.

        loop {
            self.poll_error()?;

            match &mut self.state {
                State::Start => {
                    try_ready!({
                        self.sock.as_mut()
                            .expect(POLLED_TWICE)
                            .poll_send(&self.init_segment_buf)
                    });

                    let timeout = clock::now() + RESPONSE_SEGMENT_TIMEOUT;
                    self.state = State::WaitingForResponse(Delay::new(timeout));
                },
                State::WaitingForResponse(timeout) => {
                    // If the timeout for the response has elapsed, retry
                    match timeout.poll() {
                        Ok(Async::Ready(_)) => {
                            self.state = State::Start;
                            continue;
                        },
                        Ok(Async::NotReady) => {},
                        Err(e) => return Err(Error::new(ErrorKind::Other, e)),
                    }

                    // Check if the segment is valid and ACKs our first one
                    let response = match try_ready!(self.poll_segment()) {
                        Ok(sgmt) => sgmt,
                        Err(_) => {
                            self.state = State::Start;
                            continue;
                        },
                    };
                    if !response.is_syn2_and_acks(self.local_seq_no.0) {
                        self.state = State::Start;
                        continue;
                    }

                    // Create the actual stream
                    let stream = SutpStream::create(
                        self.recv.take().expect(POLLED_TWICE),
                        self.sock.take().expect(POLLED_TWICE),
                        self.local_seq_no,
                        Wrapping(response.seq_no),
                        response.select_compression_alg(),
                    );

                    return Ok(Async::Ready(stream));
                },
            }
        }
    }
}
