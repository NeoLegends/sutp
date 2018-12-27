//! Implements the future that accepts a new connection.

use crate::{
    chunk::{Chunk, CompressionAlgorithm},
    segment::{Segment, SegmentBuilder},
    stream::SutpStream,
    CONNECTION_TIMEOUT, DRIVER_AWAY, MISSING_SEGMENT, RESPONSE_SEGMENT_TIMEOUT,
};
use bytes::Bytes;
use futures::{prelude::*, sync::mpsc, try_ready};
use log::{debug, trace};
use rand;
use std::{
    io::{self, Error, ErrorKind},
    net::SocketAddr,
};
use tokio::{clock, timer::Delay};

const POLLED_TWICE: &str = "cannot poll Accept twice";

/// A future representing an SUTP stream being accepted.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Accept {
    /// The segment, ACKing the first, in serialized form.
    ack_segment: Option<Bytes>,

    /// The compression algorithm, if one was negotiated.
    compression_algorithm: Option<CompressionAlgorithm>,

    /// The timeout guarding the whole connection setup.
    ///
    /// When this elapses the future will resolve with an error.
    conn_timeout: Option<Delay>,

    /// The current local sequence number.
    local_seq_no: u32,

    /// The channel of incoming segments.
    recv: Option<mpsc::Receiver<Result<Segment, io::Error>>>,

    /// The address of the remote endpoint.
    remote_addr: SocketAddr,

    /// The current remote sequence number.
    remote_seq_no: u32,

    /// The channel to send outgoing segments over.
    send: mpsc::Sender<(Bytes, SocketAddr)>,

    /// The channel to notify the driver about the shutdown of this stream over.
    shutdown_tx: mpsc::UnboundedSender<SocketAddr>,

    /// The internal automaton state.
    state: State,
}

/// The internal state of the accept future automaton.
#[derive(Debug)]
enum State {
    /// The connection future was created, and the response segment needs to be sent.
    Start,

    /// The response segment has been sent and we're waiting for the response to that.
    WaitingForResponse(Delay),
}

impl Accept {
    /// Constructs a future accepting an SUTP stream.
    ///
    /// `recv` is expected to eventually contain the initial SYN-> segment.
    pub(crate) fn from_listener(
        addr: SocketAddr,
        recv: mpsc::Receiver<Result<Segment, Error>>,
        send: mpsc::Sender<(Bytes, SocketAddr)>,
        on_shutdown: mpsc::UnboundedSender<SocketAddr>,
    ) -> Self {
        Self {
            ack_segment: None,
            compression_algorithm: None,
            conn_timeout: None,
            local_seq_no: rand::random(),
            recv: Some(recv),
            remote_addr: addr,
            remote_seq_no: 0,
            send,
            shutdown_tx: on_shutdown,
            state: State::Start,
        }
    }

    /// Sets up the connection timeout if it's not existing yet, and
    /// returns an error if it has elapsed.
    ///
    /// # Panics
    ///
    /// Panics (probably) if not called within a future's task context.
    fn poll_connection_timeout(&mut self) -> io::Result<()> {
        if self.conn_timeout.is_none() {
            let elapsed_at = clock::now() + CONNECTION_TIMEOUT;
            self.conn_timeout = Some(Delay::new(elapsed_at));
        }

        let fut = self.conn_timeout.as_mut().unwrap();
        match fut.poll() {
            Ok(Async::Ready(_)) => Err(ErrorKind::TimedOut.into()),
            Ok(Async::NotReady) => Ok(()),
            Err(e) => Err(Error::new(ErrorKind::Other, e)),
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
            .map(|maybe_segment| maybe_segment.expect(MISSING_SEGMENT));
        Ok(val)
    }

    /// Sends out the response.
    fn poll_send(&mut self) -> Poll<(), Error> {
        loop {
            let buf = self.ack_segment.as_ref().unwrap().clone();

            let poll_res = self
                .send
                .start_send((buf, self.remote_addr))
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

    /// Sets up the response segment ACKing the first.
    ///
    /// This is only done once, on the first poll of the future.
    fn poll_setup_response(&mut self) -> Poll<(), Error> {
        let segment = try_ready!(self.poll_segment())?;

        // First properly assign remote state and inspect their use of compression
        self.remote_seq_no = segment.seq_no;
        self.compression_algorithm = segment.select_compression_alg();

        // Build a SYN+ACK response
        // TODO: Use a real value for the window size
        let mut builder = SegmentBuilder::new()
            .seq_no(self.local_seq_no)
            .window_size(1024 * 16)
            .with_chunk(Chunk::Syn)
            .with_chunk(Chunk::Sack(self.remote_seq_no, Vec::new()));
        if let Some(alg) = self.compression_algorithm {
            builder = builder.with_chunk(alg.into_chunk());
        }

        self.ack_segment = Some(builder.build().to_vec().into());
        Ok(Async::Ready(()))
    }
}

impl Future for Accept {
    type Item = SutpStream;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if self.ack_segment.is_none() {
            try_ready!(self.poll_setup_response());
        }

        loop {
            trace!("accept poll loop");

            // Ensure we don't take too long to set up the connection
            self.poll_connection_timeout()?;

            // This implementation ping-pongs itself between the two possible states
            // of the future. In the first one, we send the <-SYN segment, and in
            // the second one we wait for and process the response.

            match &mut self.state {
                State::Start => {
                    try_ready!(self.poll_send());

                    let ack_elapsed_at = clock::now() + RESPONSE_SEGMENT_TIMEOUT;
                    self.state =
                        State::WaitingForResponse(Delay::new(ack_elapsed_at));
                }
                State::WaitingForResponse(timeout) => {
                    match timeout
                        .poll()
                        .map_err(|e| Error::new(ErrorKind::Other, e))?
                    {
                        Async::Ready(_) => {
                            trace!("timeout for accept elapsed, re-sending...");

                            self.state = State::Start;
                            continue;
                        }
                        Async::NotReady => {}
                    }

                    trace!("polling for segment");

                    let ack_segment = try_ready!(self.poll_segment())?;

                    trace!("segment recved");

                    // Check if the received segment is the clients second segment
                    // for proper remote_seq_no begin
                    if ack_segment.seq_no != self.remote_seq_no.wrapping_add(1) {
                        debug!(
                            "response seq no {} does not match expected one {}",
                            ack_segment.seq_no,
                            self.remote_seq_no.wrapping_add(1)
                        );

                        self.remote_seq_no = ack_segment.seq_no;
                        self.state = State::Start;
                        continue;
                    } else if ack_segment.ack(self.local_seq_no).is_nak() {
                        // Retry if the other side didn't receive the segment properly

                        debug!("syn2 NAKed {:?}", ack_segment.chunks);

                        self.state = State::Start;
                        continue;
                    }

                    let seq_no = ack_segment.seq_no;
                    let window_size = ack_segment.window_size;

                    // Use a temporary channel to put the received segment back
                    // into a channel. This simplifies the implementation in the
                    // stream.
                    let (tx, rx) = {
                        let (mut tx, rx) = mpsc::channel(0);

                        // this can only fail when allocations fail
                        tx.try_send(Ok(ack_segment)).unwrap();

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

                    trace!("building up stream");

                    // Build up the actual stream and resolve the future
                    let stream = SutpStream::create(
                        rx,
                        self.send.clone(),
                        self.shutdown_tx.clone(),
                        self.local_seq_no,
                        self.remote_addr,
                        seq_no,
                        window_size,
                        self.compression_algorithm,
                    );
                    return Ok(Async::Ready(stream));
                }
            }
        }
    }
}
