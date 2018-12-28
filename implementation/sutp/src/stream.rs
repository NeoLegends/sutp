use crate::{
    chunk::{Chunk, CompressionAlgorithm},
    connect::Connect,
    segment::{Segment, SegmentBuilder},
    window::{InsertError, Window},
    CONNECTION_TIMEOUT, DRIVER_AWAY, RECEIVER_ERROR, RESPONSE_SEGMENT_TIMEOUT,
};
use bytes::{Buf, BufMut, Bytes, BytesMut, IntoBuf};
use futures::{prelude::*, sync::mpsc, try_ready};
use log::{debug, trace};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Error, ErrorKind, Read, Write},
    mem,
    net::SocketAddr,
};
use tokio::{
    clock,
    io::{AsyncRead, AsyncWrite},
    timer::Delay,
};

/// Converts from Async::NotReady to ErrorKind::WouldBlock.
macro_rules! try_would_block {
    ($val:expr) => {{
        match $val? {
            Async::Ready(result) => Ok(result),
            Async::NotReady => Err(ErrorKind::WouldBlock.into()),
        }
    }};
}

/// The default size of the receiving and sending buffers.
const BUF_SIZE: usize = 1024 * 1024; // 1MB

/// The default size of outgoing payload chunks (1k).
const OUTGOING_PAYLOAD_SIZE: usize = 1024;

/// A full-duplex SUTP stream.
///
/// This struct implements the tokio `AsyncRead` and `AsyncWrite`-family of traits
/// to allow for asynchronous data transfer over the network.
#[derive(Debug)]
pub struct SutpStream {
    /// The negotiated compression algorithm.
    compression_algorithm: Option<CompressionAlgorithm>,

    /// The highest consecutively received sequence number.
    ///
    /// I. e., this is not the sequence number of the segment we expect next, but
    /// the one we currently have.
    highest_consecutive_remote_seq_no: u32,

    /// The highest sent-out local sequence number.
    ///
    /// I. e., this is the sequence number that was last sent, not the one we
    /// send next.
    highest_sent_local_seq_no: u32,

    /// The list of sequence numbers of segments that could not be
    /// received properly.
    nak_set: BTreeSet<u32>,

    /// The channel to push the shutdown of this stream to.
    on_shutdown: mpsc::UnboundedSender<SocketAddr>,

    /// The sparse buffer for bringing segments into their proper order.
    order_buf: Window<Segment>,

    /// Segments which have to be transferred and ACKed.
    outgoing_segments: BTreeMap<u32, Outgoing>,

    /// The read buffer to buffer successfully received and ordered segment
    /// data into.
    r_buf: BytesMut,

    /// A receiver with incoming segments.
    recv: mpsc::Receiver<Result<Segment, io::Error>>,

    /// The address of the remote endpoint.
    remote_addr: SocketAddr,

    /// The last-known remaining window size.
    remote_window_size: u32,

    /// The buffer to temporarily write serialized segment contents to.
    segment_buf: BytesMut,

    /// The channel used to send outgoing segments to the driver.
    send: mpsc::Sender<(Bytes, SocketAddr)>,

    /// The state of the stream.
    state: StreamState,

    /// The buffer that user data to transmit is stored in until it is
    /// divided into segments (which are then stored in `segment_buf`).
    w_buf: BytesMut,
}

/// The representation of an outgoing segment that has not-yet been ACKed.
#[derive(Debug)]
struct Outgoing {
    /// The segment in serialized binary form.
    pub data: Bytes,

    /// A timeout that guards the transfer of the entire segment.
    ///
    /// If this is `None`, the segment has not-yet been sent.
    ///
    /// When this elapses, the entire connection times out and will be shut down.
    fail_timeout: Option<Delay>,

    /// A timeout that guards one round-trip of the segment + its ACK.
    ///
    /// If this is `None`, the segment has not-yet been sent.
    ///
    /// When this elapses the segment is simply re-sent.
    resend_timeout: Option<Delay>,
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

    /// A FIN chunk has been received, and we are currently sending out our own.
    LastBreath,

    /// The connection is closed and no further data can be sent.
    Closed,
}

impl SutpStream {
    /// Creates an SUTP connection to the given address.
    ///
    /// When the returned future completes, the stream has been established
    /// and can be used to transmit data.
    pub fn connect(addr: &SocketAddr) -> Connect {
        Connect::new(addr)
    }

    /// Constructs a new stream from the given connection primitives.
    ///
    /// This is the internally-used function to create an instance of the stream
    /// after either accepting or creating a new connection.
    ///
    /// The remote_seq_no is the highest consecutively received one, i. e. not the
    /// one we expect next, but the one we currently have.
    pub(crate) fn create(
        recv: mpsc::Receiver<Result<Segment, Error>>,
        send: mpsc::Sender<(Bytes, SocketAddr)>,
        on_shutdown: mpsc::UnboundedSender<SocketAddr>,
        local_seq_no: u32,
        remote_addr: SocketAddr,
        remote_seq_no: u32,
        remote_win_size: u32,
        compression_alg: Option<CompressionAlgorithm>,
    ) -> Self {
        Self {
            compression_algorithm: compression_alg,
            highest_consecutive_remote_seq_no: remote_seq_no,
            highest_sent_local_seq_no: local_seq_no,
            nak_set: BTreeSet::new(),
            on_shutdown,
            order_buf: Window::with_lowest_key(
                BUF_SIZE / OUTGOING_PAYLOAD_SIZE,
                remote_seq_no as usize,
            ),
            outgoing_segments: BTreeMap::new(),
            r_buf: BytesMut::with_capacity(BUF_SIZE),
            recv,
            remote_addr,
            remote_window_size: remote_win_size,
            // preallocation doesn't really matter here
            segment_buf: BytesMut::new(),
            send,
            state: StreamState::Open,
            w_buf: BytesMut::with_capacity(BUF_SIZE),
        }
    }

    /// Asserts that the stream is in the proper state to read or flush.
    fn assert_can_flush(&self) -> io::Result<()> {
        match self.state {
            StreamState::Closed => Err(ErrorKind::NotConnected.into()),
            _ => Ok(()),
        }
    }

    /// Asserts that the stream is in the proper state to write.
    fn assert_can_write(&self) -> io::Result<()> {
        match self.state {
            StreamState::Open | StreamState::FinRecvd => Ok(()),
            _ => Err(ErrorKind::NotConnected.into()),
        }
    }

    /// Fills the given buffer with as much read data as possible.
    fn fill_buf(&mut self, buf: &mut [u8]) -> usize {
        let copy_count = buf.len().min(self.r_buf.len());
        self.r_buf
            .split_to(copy_count)
            .freeze()
            .into_buf()
            .copy_to_slice(&mut buf[..copy_count]);

        copy_count
    }

    /// Gets a fresh sequence number by first incrementing the stored one and
    /// returning the incremented value.
    fn get_seq_no(&mut self) -> u32 {
        self.highest_sent_local_seq_no =
            self.highest_sent_local_seq_no.wrapping_add(1);
        self.highest_sent_local_seq_no
    }

    /// Asynchronously tries to read data off the stream.
    fn poll_read(&mut self, buf: &mut [u8]) -> Poll<usize, io::Error> {
        if buf.is_empty() {
            return Ok(Async::Ready(0));
        }

        while {
            match self.state {
                StreamState::Closed | StreamState::FinRecvd => {
                    return Ok(Async::Ready(self.fill_buf(buf)))
                }
                _ => {}
            }

            self.poll_process()?
        } {}

        Ok(if !self.r_buf.is_empty() {
            Async::Ready(self.fill_buf(buf))
        } else {
            Async::NotReady
        })
    }

    /// Asynchronously tries to write data to the stream.
    ///
    /// Note that until `.poll_flush()` is called, no data is actually written
    /// to the network.
    fn poll_write(&mut self, buf: &[u8]) -> Poll<usize, io::Error> {
        self.assert_can_write()?;

        trace!("writing {} bytes", buf.len());

        if buf.is_empty() {
            return Ok(Async::Ready(0));
        } else if self.w_buf.remaining_mut() == 0 {
            return Ok(Async::NotReady);
        }

        let copy_count = self.w_buf.remaining_mut().min(buf.len());
        self.w_buf.put_slice(&buf[..copy_count]);

        Ok(Async::Ready(copy_count))
    }

    /// Asynchronously tries to flush the stream, sending the data over the wire.
    ///
    /// This function does not wait for the sent segments to be acked.
    fn poll_flush(&mut self) -> Poll<(), io::Error> {
        self.assert_can_flush()?;

        while {
            self.prepare_segments();

            // Gah I hate this style, but functional combinators don't allow
            // modifications to the control flow of this function and we need
            // those to handle erros and non-readyness.
            for outgoing in self.outgoing_segments.values_mut() {
                match outgoing.should_send() {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        self.state = StreamState::Closed;
                        return Err(e);
                    }
                }
                if (self.remote_window_size as usize) < outgoing.data.len() {
                    break;
                }

                // Send the segment to the driver
                loop {
                    let poll_res = self
                        .send
                        .start_send((outgoing.data.clone(), self.remote_addr))
                        .map_err(|_| Error::new(ErrorKind::Other, DRIVER_AWAY));

                    if poll_res?.is_ready() {
                        break;
                    }

                    try_ready!({
                        self.send
                            .poll_complete()
                            .map_err(|_| Error::new(ErrorKind::Other, DRIVER_AWAY))
                    });
                }

                outgoing.start_timers();
                self.remote_window_size
                    .saturating_sub(outgoing.data.len() as u32);
            }

            self.poll_process()?
        } {}

        Ok(Async::Ready(()))
    }

    /// Asynchronously drives the protocol automaton receiving new segments,
    /// preparing ACK segments for received ones, etc.
    fn poll_process(&mut self) -> Result<bool, io::Error> {
        let mut has_recvd_segments = false;

        loop {
            // Check for new segments on the channel
            let segment = match self.recv.poll().expect(RECEIVER_ERROR) {
                Async::Ready(Some(Ok(segment))) => {
                    has_recvd_segments = true;
                    segment
                }
                Async::Ready(Some(Err(e))) => {
                    debug!("received invalid segment {:?}", e);
                    continue;
                }
                Async::Ready(None) => {
                    return Err(Error::new(ErrorKind::Other, DRIVER_AWAY))
                }
                Async::NotReady => {
                    break;
                }
            };

            trace!("received segment {}", segment.seq_no);

            // Store the segment itself for later removal. If the buffer is full,
            // at it to the NAK set.
            match self.order_buf.insert(segment.seq_no as usize, segment) {
                Ok(_) => {}
                Err(InsertError::DistanceTooLarge(s)) => {
                    self.nak_set.insert(s.seq_no);
                }
                Err(InsertError::KeyTooLow(s)) => {
                    trace!("already seen segment {}", s.seq_no);
                    continue;
                }
                Err(InsertError::WouldOverwrite(s)) => {
                    self.nak_set.insert(s.seq_no);
                }
            }
        }

        let mut highest_seq_no = None;

        // Track if we have seen other chunks than SACK chunks, because segments
        // containing just these won't be ACKed.
        let mut needs_ack = false;

        for segment in self.order_buf.drain() {
            trace!(
                "processing segment {} with {:?}",
                segment.seq_no,
                segment.chunks
            );

            highest_seq_no = Some(segment.seq_no);
            needs_ack |= segment.needs_ack();

            // Remove segment from NAK list since it's been successfully received
            self.nak_set.remove(&segment.seq_no);

            // We need to take ownership of our outgoing_segments map below, but
            // this cannot be done in a mutable context. Thus, we temporarily
            // replace the stored map with an empty one (this doesn't allocate!)
            // to be able to do the filtering.
            let segments =
                mem::replace(&mut self.outgoing_segments, BTreeMap::new());

            // Remove ACKed segments from out outgoing list
            self.outgoing_segments = segments
                .into_iter()
                .filter(|(seq_no, _)| !segment.ack(*seq_no).is_ack())
                .collect();

            // Trigger immediate re-send for all outgoing NAKed segments
            self.outgoing_segments
                .iter_mut()
                .filter(|(seq_no, _)| segment.ack(**seq_no).is_nak())
                .for_each(|(_, outgoing)| outgoing.send_immediately());

            for chunk in segment.chunks {
                match chunk {
                    Chunk::Abort => {
                        trace!("received ABRT chunk");

                        self.outgoing_segments.clear();
                        self.r_buf.clear();
                        self.w_buf.clear();

                        self.state = StreamState::Closed;

                        return Err(ErrorKind::ConnectionAborted.into());
                    }
                    Chunk::Fin => {
                        trace!("received FIN chunk");

                        self.state = match self.state {
                            StreamState::FinSent | StreamState::LastBreath => {
                                StreamState::Closed
                            }
                            _ => StreamState::FinRecvd,
                        };
                    }
                    Chunk::Payload(payload) => {
                        trace!("received payload chunk");

                        // TODO: This loses data if r_buf has too little space!
                        let copy_count =
                            self.r_buf.remaining_mut().min(payload.len());
                        self.r_buf.put_slice(&payload[..copy_count]);
                    }
                    _ => {}
                }
            }
        }

        if let Some(seq_no) = highest_seq_no {
            self.highest_consecutive_remote_seq_no = seq_no;

            // Set the next expected sequence number in our sparse buffer to prevent
            // data loss by segments arriving in the wrong order preventing earlier
            // segments that have not yet been received to be inserted.
            if self.order_buf.is_empty() {
                self.order_buf
                    .set_lowest_key(seq_no.wrapping_add(1) as usize)
            }
        }

        // Build up an ACK / NAK segment
        if needs_ack {
            let nak_list = self.nak_set.iter().cloned().collect();
            let ack_nak_segment = SegmentBuilder::new()
                .seq_no(self.get_seq_no())
                .window_size(self.w_buf.remaining_mut() as u32)
                .with_chunk(Chunk::Sack(
                    self.highest_consecutive_remote_seq_no,
                    nak_list,
                ))
                .build();

            trace!(
                "enqueueing ack segment {} for {:?}",
                ack_nak_segment.seq_no,
                ack_nak_segment.chunks
            );
            self.enqueue_outgoing(&ack_nak_segment);
        }

        Ok(has_recvd_segments)
    }

    /// Creates segments of optimal size out of the buffer that contains
    /// user data.
    ///
    /// Note: This gives us packet bundling "for free", at least depending on
    /// how often the user flushes the stream.
    fn prepare_segments(&mut self) {
        while !self.w_buf.is_empty() {
            // Get the data we want to send out with a single segment.
            let num_payload_bytes = self.w_buf.len().min(OUTGOING_PAYLOAD_SIZE);
            let payload = self.w_buf.split_to(num_payload_bytes);

            // Right now, creating the binary segment is implemented inefficiently.
            // We first copy the slice to be sent to our buffer (in poll_read),
            // split it off here, pass it as owned Bytes to the payload chunk, and
            // then write the segment as well to our buffer to again split it off
            // and store it in here.
            //
            // This lets us avoid an allocation, but forces us to copy the actual
            // payload twice.
            //
            // In theory, we would be able to save the first copying, by somehow
            // making Chunk able to serialize itself with a borrowed payload, but
            // I can't figure out how to do that without polluting the type
            // signature too much.

            // Build the segment
            let segment = SegmentBuilder::new()
                .seq_no(self.get_seq_no())
                .window_size(self.w_buf.remaining_mut() as u32)
                .with_chunk(Chunk::Payload(payload.freeze()))
                .build();

            self.enqueue_outgoing(&segment);
        }

        // Since everything has been copied to segment_buf, this will reclaim
        // the entire buffer space without allocating.
        self.w_buf.reserve(BUF_SIZE);
    }

    /// Serializes the given segment and enqueues it for sending.
    fn enqueue_outgoing(&mut self, segment: &Segment) {
        trace!(
            "enqueueing segment {} with {:?}",
            segment.seq_no,
            segment.chunks,
        );

        self.segment_buf.reserve(segment.binary_len());

        segment
            .write_to(&mut (&mut self.segment_buf).writer())
            .unwrap(); // since we're writing to memory this is infallible

        let serialized_segment =
            self.segment_buf.split_to(self.segment_buf.len()).freeze();

        self.outgoing_segments
            .insert(segment.seq_no, Outgoing::new(serialized_segment));
    }
}

unsafe impl Send for SutpStream {}

impl Read for SutpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        try_would_block!(self.poll_read(buf))
    }
}

impl AsyncRead for SutpStream {}

impl Write for SutpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        try_would_block!(self.poll_write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        try_would_block!(self.poll_flush())
    }
}

impl AsyncWrite for SutpStream {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        loop {
            match self.state {
                s @ StreamState::Open | s @ StreamState::FinRecvd => {
                    trace!("shutdown: open | finrecvd");

                    try_ready!(self.poll_flush());

                    let fin_segment = SegmentBuilder::new()
                        .seq_no(self.get_seq_no())
                        .window_size(self.w_buf.remaining_mut() as u32)
                        .with_chunk(Chunk::Fin)
                        .build();

                    self.enqueue_outgoing(&fin_segment);
                    self.state = match s {
                        StreamState::Open => StreamState::FinSent,
                        StreamState::FinRecvd => StreamState::LastBreath,
                        _ => unreachable!(),
                    };
                }
                StreamState::FinSent | StreamState::LastBreath => {
                    trace!("shutdown: fin sent | last breath");

                    try_ready!(self.poll_flush());
                    self.state = StreamState::Closed;
                }
                StreamState::Closed => {
                    trace!("shutdown: closed");
                    return Ok(Async::Ready(()));
                }
            }
        }
    }
}

impl Drop for SutpStream {
    fn drop(&mut self) {
        self.recv.close();
        let _ = self.send.close();
        let _ = self.on_shutdown.unbounded_send(self.remote_addr);
    }
}

impl Outgoing {
    /// Constructs a new `Outgoing`.
    pub fn new(buf: Bytes) -> Self {
        Outgoing {
            data: buf,
            fail_timeout: None,
            resend_timeout: None,
        }
    }

    /// Checks whether the segment should to be sent over the wire.
    ///
    /// Returns Ok(true) if the segment should to be sent, Ok(false) otherwise.
    /// Returns Err(_) when either the timer system fails, or if the failure
    /// timeout has elapsed.
    pub fn should_send(&mut self) -> Result<bool, Error> {
        if let Some(fut) = self.fail_timeout.as_mut() {
            match fut.poll() {
                Ok(Async::Ready(_)) => return Err(ErrorKind::TimedOut.into()),
                Ok(Async::NotReady) => {}
                Err(e) => return Err(Error::new(ErrorKind::Other, e)),
            }
        }
        if let Some(fut) = self.resend_timeout.as_mut() {
            return match fut.poll() {
                Ok(Async::Ready(_)) => Ok(true),
                Ok(Async::NotReady) => Ok(false),
                Err(e) => Err(Error::new(ErrorKind::Other, e)),
            };
        }

        Ok(true)
    }

    /// Sets the segment to be immediately sent again.
    pub fn send_immediately(&mut self) {
        self.resend_timeout = None;
    }

    /// (Re)starts the timers guarding the segment transmission.
    ///
    /// Once started, the failure timer is not restarted.
    pub fn start_timers(&mut self) {
        if self.fail_timeout.is_none() {
            let elapse_at = clock::now() + CONNECTION_TIMEOUT;
            self.fail_timeout = Some(Delay::new(elapse_at));
        }

        let elapse_at = clock::now() + RESPONSE_SEGMENT_TIMEOUT;
        self.resend_timeout = Some(Delay::new(elapse_at));
    }
}
