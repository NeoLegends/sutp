//! Implements the future that builds up a new connection.

use crate::{
    chunk::{Chunk, SUPPORTED_COMPRESSION_ALGS},
    driver::{Driver, NEW_CONN_QUEUE_SIZE},
    segment::{Segment, SegmentBuilder},
    stream::SutpStream,
    CONNECTION_TIMEOUT, RESPONSE_SEGMENT_TIMEOUT,
};
use bytes::Bytes;
use futures::{
    prelude::*,
    sync::{mpsc, oneshot},
    try_ready,
};
use lazy_static::lazy_static;
use log::trace;
use rand;
use std::{
    io::{Error, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::Wrapping,
};
use tokio::{clock, net::UdpSocket, timer::Delay};

const DRIVER_AWAY: &str = "driver has gone away";
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
    init_segment_buf: Bytes,

    /// The local sequence number.
    local_seq_no: Wrapping<u32>,

    /// The channel of incoming segments.
    recv: Option<mpsc::Receiver<Result<Segment, Error>>>,

    /// The remote socket address.
    remote_addr: SocketAddr,

    /// The remote sequence number.
    remote_seq_no: Wrapping<u32>,

    /// The channel to send outgoing segments to the driver.
    send: mpsc::Sender<(Bytes, SocketAddr)>,

    /// The channel to notify the driver about the shutdown of this stream over.
    shutdown_tx: mpsc::UnboundedSender<SocketAddr>,

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
        let (from_driver_tx, from_driver_rx) = mpsc::channel(NEW_CONN_QUEUE_SIZE);
        let (to_driver_tx, to_driver_rx) = mpsc::channel(NEW_CONN_QUEUE_SIZE);
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded();

        // Spawn a driver for this connection
        let driver = Driver::from_connection(
            UdpSocket::bind(&BIND_ADDR)?,
            err_tx,
            addr,
            from_driver_tx,
            to_driver_rx,
            shutdown_rx,
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

        Ok(Self {
            io_err: Some(err_rx),
            init_segment_buf: initial_sgmt_buf.into(),
            local_seq_no: seq_no,
            recv: Some(from_driver_rx),
            remote_addr: *addr,
            remote_seq_no: Wrapping(0),
            send: to_driver_tx,
            shutdown_tx,
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
            Err(_) => Err(Error::new(ErrorKind::Other, DRIVER_AWAY)),
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
        let val = self
            .recv
            .as_mut()
            .expect(POLLED_TWICE)
            .poll()
            .expect(DRIVER_AWAY)
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

    /// Sends the initial segment to the driver.
    fn poll_send(&mut self) -> Poll<(), Error> {
        loop {
            let poll_res = self
                .send
                .start_send((self.init_segment_buf.clone(), self.remote_addr))
                .map_err(|_| Error::new(ErrorKind::Other, DRIVER_AWAY));

            match poll_res? {
                AsyncSink::Ready => return Ok(Async::Ready(())),
                AsyncSink::NotReady(_) => try_ready!({
                    self.send
                        .poll_complete()
                        .map_err(|_| Error::new(ErrorKind::Other, DRIVER_AWAY))
                }),
            }
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
                    try_ready!(self.poll_send());

                    let timeout = clock::now() + RESPONSE_SEGMENT_TIMEOUT;
                    self.state = State::WaitingForResponse(Delay::new(timeout));
                }
                State::WaitingForResponse(timeout) => {
                    // If the timeout for the response has elapsed, retry
                    match timeout.poll() {
                        Ok(Async::Ready(_)) => {
                            self.state = State::Start;
                            continue;
                        }
                        Ok(Async::NotReady) => {}
                        Err(e) => return Err(Error::new(ErrorKind::Other, e)),
                    }

                    // Check if the segment is valid and ACKs our first one
                    let response = match try_ready!(self.poll_segment()) {
                        Ok(sgmt) => sgmt,
                        Err(e) => {
                            trace!(
                                "received invalid segment during connection: {:?}",
                                e
                            );

                            self.state = State::Start;
                            continue;
                        }
                    };
                    if !response.is_syn2_and_acks(self.local_seq_no.0) {
                        self.state = State::Start;
                        continue;
                    }

                    let compression_alg = response.select_compression_alg();
                    let seq_no = response.seq_no;
                    let window_size = response.window_size;

                    // Use a temporary channel to put the received segment back
                    // into a channel. This simplifies the implementation in the
                    // stream.
                    let (tx, rx) = {
                        let (mut tx, rx) = mpsc::channel(0);
                        tx.try_send(Ok(response))
                            .expect("failed to re-enqueue 3rd segment");

                        let tx = tx.sink_map_err(|_| {
                            trace!("intermediate channel dropped")
                        });

                        (tx, rx)
                    };
                    tokio::spawn(
                        self.recv
                            .take()
                            .expect(POLLED_TWICE)
                            .forward(tx)
                            .map(|_| ()),
                    );

                    // Create the actual stream
                    let stream = SutpStream::create(
                        rx,
                        self.send.clone(),
                        self.shutdown_tx.clone(),
                        self.local_seq_no.0,
                        self.remote_addr,
                        seq_no,
                        window_size,
                        compression_alg,
                    );

                    return Ok(Async::Ready(stream));
                }
            }
        }
    }
}
