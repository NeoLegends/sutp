//! Implements the future that accepts a new connection.

use crate::{
    CONNECTION_TIMEOUT,
    RESPONSE_SEGMENT_TIMEOUT,
    chunk::{Chunk, CompressionAlgorithm},
    segment::{Segment, SegmentBuilder},
    stream::SutpStream,
};
use futures::{
    prelude::*,
    sync::mpsc,
    try_ready,
};
use rand;
use std::{
    io::{self, Error, ErrorKind},
    num::Wrapping,
};
use tokio::{
    clock,
    net::udp::UdpSocket,
    timer::Delay,
};

const DRIVER_AWAY: &str = "driver has gone away";
const MISSING_SGMT: &str = "missing segment";
const POLLED_TWICE: &str = "cannot poll Accept twice";

/// A future representing an SUTP stream being accepted.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Accept {
    /// The segment, ACKing the first, in serialized form and its sequence number.
    ack_segment: Option<Vec<u8>>,

    /// The timeout guarding the send / response of a single ACK response.
    ///
    /// When this elapses, the response will be re-sent.
    ack_timeout: Option<Delay>,

    /// The compression algorithm, if one was negotiated.
    compression_algorithm: Option<CompressionAlgorithm>,

    /// The timeout guarding the whole connection setup.
    ///
    /// When this elapses the future will resolve with an error.
    conn_timeout: Option<Delay>,

    /// The current local sequence number.
    local_seq_no: Wrapping<u32>,

    /// The channel of incoming segments.
    recv: Option<mpsc::Receiver<Result<Segment, io::Error>>>,

    /// The current remote sequence number.
    remote_seq_no: Wrapping<u32>,

    /// The socket to send segments over.
    send_socket: Option<UdpSocket>,
}

impl Accept {
    /// Constructs a future accepting an SUTP stream.
    ///
    /// `recv` is expected to eventually contain the initial SYN-> segment.
    pub(crate) fn from_listener(
        sock: UdpSocket,
        recv: mpsc::Receiver<Result<Segment, Error>>,
    ) -> Self {
        Self {
            ack_segment: None,
            ack_timeout: None,
            compression_algorithm: None,
            conn_timeout: None,
            local_seq_no: Wrapping(rand::random()),
            recv: Some(recv),
            remote_seq_no: Wrapping(0),
            send_socket: Some(sock),
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
        let val = self.recv.as_mut()
            .expect(POLLED_TWICE)
            .poll()
            .expect(DRIVER_AWAY)
            .map(|maybe_segment| maybe_segment.expect(MISSING_SGMT));
        Ok(val)
    }

    /// Polls the delay future for a single segment.
    ///
    /// This also resets the delay future when it has completed.
    fn poll_segment_timeout(&mut self) -> Poll<(), Error> {
        if let Some(ref mut delay) = self.ack_timeout {
            match delay.poll() {
                Ok(Async::Ready(_)) => {},
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(e) => return Err(Error::new(ErrorKind::Other, e)),
            }
        }

        self.ack_timeout = None;
        Ok(Async::Ready(()))
    }

    /// Sets up the response segment ACKing the first.
    ///
    /// This is only done once, on the first poll of the future.
    fn poll_setup_response(&mut self) -> Poll<(), Error> {
        let segment = try_ready!(self.poll_segment())?;

        // First properly assign remote state and inspect their use of compression
        self.remote_seq_no = Wrapping(segment.seq_no);
        self.compression_algorithm = segment.select_compression_alg();

        // Build a SYN+ACK response
        // TODO: Use a real value for the window size
        let mut builder = SegmentBuilder::new()
            .seq_no(self.local_seq_no.0)
            .window_size(1024 * 16)
            .with_chunk(Chunk::Syn)
            .with_chunk(Chunk::Sack(self.remote_seq_no.0, Vec::new()));
        if let Some(alg) = self.compression_algorithm {
            builder = builder.with_chunk(alg.into_chunk());
        }

        self.ack_segment = Some(builder.build().to_vec());
        Ok(Async::Ready(()))
    }

    /// Sets the timeout for a single ACK RTT.
    fn set_segment_timeout(&mut self) {
        let ack_elapsed_at = clock::now() + RESPONSE_SEGMENT_TIMEOUT;
        self.ack_timeout = Some(Delay::new(ack_elapsed_at));
    }
}

impl Future for Accept {
    type Item = SutpStream;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        // Ensure we don't take too long to set up the connection
        self.poll_connection_timeout()?;

        loop {
            if self.ack_segment.is_none() {
                try_ready!(self.poll_setup_response());
            }

            let ack_segment_buf = self.ack_segment.clone().unwrap();
            let maybe_response = self.poll_segment()?;

            // See if the other side has answered
            if maybe_response.is_not_ready() {
                // Only send if we're allowed to
                try_ready!(self.poll_segment_timeout());

                // If not, send the segment and set a timeout when to try again
                let sock = self.send_socket.as_mut().expect(POLLED_TWICE);
                try_ready!(sock.poll_send(&ack_segment_buf));

                self.set_segment_timeout();
                return Ok(Async::NotReady);
            }

            // ...and if the answer is valid
            let ack_segment = match maybe_response {
                // TODO: Ignore faulty response?
                Async::Ready(maybe_segment) => maybe_segment?,
                _ => unreachable!(),
            };

            // Check if the received segment is the clients second segment
            // for proper remote_seq_no begin
            if ack_segment.seq_no != self.remote_seq_no.0 + 1 {
                self.ack_timeout = None;
                continue;
            }

            // Retry if the other side didn't receive the segment properly
            if !ack_segment.ack(self.local_seq_no.0).is_ack() {
                self.ack_timeout = None;
                continue;
            }

            self.remote_seq_no += Wrapping(1);
            let window_size = ack_segment.window_size;

            // Use a temporary channel to put the received segment back
            // into a channel. This simplifies the implementation in the
            // stream.
            let (tx, rx) = {
                let (mut tx, rx) = mpsc::channel(0);
                tx.try_send(Ok(ack_segment))
                    .expect("failed to re-enqueue 3rd segment");

                (tx.sink_map_err(|e| panic!("channel tx err: {:?}", e)), rx)
            };
            tokio::spawn(
                self.recv.take()
                    .expect(POLLED_TWICE)
                    .forward(tx)
                    .map(|_| ())
            );

            // Build up the actual stream and resolve the future
            let stream = SutpStream::create(
                rx,
                self.send_socket.take().expect(POLLED_TWICE),
                self.local_seq_no + Wrapping(1),
                self.remote_seq_no,
                window_size,
                self.compression_algorithm,
            );
            return Ok(Async::Ready(stream));
        }
    }
}
