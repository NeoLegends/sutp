use crate::{
    chunk::{Chunk, CompressionAlgorithm},
    connect::Connect,
    segment::{Segment, SegmentBuilder},
    sparse_buf::{InsertError, SparseBuffer},
    CONNECTION_TIMEOUT, RESPONSE_SEGMENT_TIMEOUT,
};
use bytes::{Buf, BufMut, Bytes, BytesMut, IntoBuf};
use futures::{prelude::*, sync::mpsc, try_ready};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Error, ErrorKind, Read, Write},
    net::SocketAddr,
    num::Wrapping,
};
use tokio::{
    clock,
    io::{AsyncRead, AsyncWrite},
    net::udp::UdpSocket,
    timer::Delay,
};

/// The default size of the receiving and sending buffers.
const BUF_SIZE: usize = 1024 * 1024 * 1; // 1MB

/// The 1 as wrapping u32.
const ONE: Wrapping<u32> = Wrapping(1);

/// The default size of outgoing payload chunks (1k).
const OUTGOING_PAYLOAD_SIZE: usize = 1024;

/// The minimum window size needed to still allow data transmits.
///
/// If the reported window size is below this constant, we apply backpressure and
/// wait until the receiver informs us about more space left (we can reasonably
/// assume that receivers will have a larger window than 128 bytes, lol).
const MIN_OUTGOING_WINDOW_SIZE: usize = 128;

/// A full-duplex SUTP stream.
#[derive(Debug)]
pub struct SutpStream {
    /// The set of sequence numbers to ACK.
    ack_set: SparseBuffer<u32, &'static Fn(&u32) -> usize>,

    /// The negotiated compression algorithm.
    compression_algorithm: Option<CompressionAlgorithm>,

    /// The current local sequence number.
    local_seq_no: Wrapping<u32>,

    /// The list of sequence numbers of segments that could not be
    /// received properly.
    nak_set: BTreeSet<u32>,

    /// The sparse buffer for bringing segments into their proper order.
    order_buf: SparseBuffer<Segment, &'static Fn(&Segment) -> usize>,

    /// Segments which have to be transferred and ACKed.
    outgoing_segments: Option<BTreeMap<u32, Outgoing>>, // Option dance

    /// The read buffer to buffer successfully received and ordered segment
    /// data into.
    r_buf: BytesMut,

    /// A receiver with incoming segments.
    recv: mpsc::Receiver<Result<Segment, io::Error>>,

    /// The current remote sequence number.
    remote_seq_no: Wrapping<u32>,

    /// The last-known remaining window size.
    remote_window_size: u32,

    /// The UDP socket to send segments from.
    send_socket: UdpSocket,

    /// The buffer to temporarily write serialized segment contents to.
    segment_buf: BytesMut,

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
    pub(crate) fn create(
        recv: mpsc::Receiver<Result<Segment, Error>>,
        sock: UdpSocket,
        local_seq_no: Wrapping<u32>,
        remote_seq_no: Wrapping<u32>,
        remote_win_size: u32,
        compression_alg: Option<CompressionAlgorithm>,
    ) -> Self {
        /// The function used as key selector of the sparse buffer storing the
        /// ACKed sequence numbers.
        ///
        /// This needs to be a real function instead of a lambda to be 'static
        /// (we pass a &'static of this function to the sparse buffer below).
        fn ack_set_key_selector(seq_no: &u32) -> usize {
            *seq_no as usize
        }

        /// The function used as key selector of the sparse buffer storing the
        /// segments that have arrived.
        ///
        /// This needs to be a real function instead of a lambda to be 'static
        /// (we pass a &'static of this function to the sparse buffer below).
        fn order_key_selector(s: &Segment) -> usize {
            s.seq_no as usize
        }

        Self {
            ack_set: SparseBuffer::new(1024, &ack_set_key_selector),
            compression_algorithm: compression_alg,
            local_seq_no,
            nak_set: BTreeSet::new(),
            order_buf: SparseBuffer::new(
                BUF_SIZE / OUTGOING_PAYLOAD_SIZE,
                &order_key_selector,
            ),
            outgoing_segments: Some(BTreeMap::new()),
            r_buf: BytesMut::with_capacity(BUF_SIZE),
            recv,
            remote_seq_no,
            remote_window_size: remote_win_size,
            segment_buf: BytesMut::with_capacity(BUF_SIZE),
            send_socket: sock,
            state: StreamState::Open,
            w_buf: BytesMut::with_capacity(BUF_SIZE),
        }
    }

    /// Asserts that the stream is in the proper state to be able to write data
    /// to the network and to the other side.
    fn assert_can_write(&self) -> io::Result<()> {
        match self.state {
            StreamState::Open | StreamState::FinRecvd => Ok(()),
            _ => Err(ErrorKind::NotConnected.into()),
        }
    }

    /// Gets a fresh sequence number.
    fn get_seq_no(&mut self) -> u32 {
        self.local_seq_no += ONE;
        self.local_seq_no.0
    }

    /// Asynchronously tries to read data off the stream.
    fn poll_read(&mut self, buf: &mut [u8]) -> Poll<usize, io::Error> {
        match self.state {
            StreamState::Open => {}
            StreamState::FinRecvd => unimplemented!(),
            _ => return Err(ErrorKind::NotConnected.into()),
        }

        self.poll_recv()?;

        let copy_count = buf.len().min(self.r_buf.len());
        self.r_buf
            .split_to(copy_count)
            .freeze()
            .into_buf()
            .copy_to_slice(&mut buf[..copy_count]);

        Ok(Async::Ready(copy_count))
    }

    /// Asynchronously receives and processes newly arriving segments.
    fn poll_recv(&mut self) -> Result<(), io::Error> {
        loop {
            // Check for new segments on the channel
            let poll_res = self
                .recv
                .poll()
                .map_err(|_| {
                    io::Error::new(ErrorKind::Other, "driver has gone away")
                })?
                .map(|maybe| maybe.expect("missing segment"));
            let segment = match poll_res {
                Async::Ready(Ok(segment)) => segment,
                Async::Ready(Err(_)) => continue,
                Async::NotReady => break,
            };

            // Store the sequence number in our ACK list. If that's full,
            // insert it into our NAK set (this is unbounded at the moment).
            match self.ack_set.push(segment.seq_no) {
                Ok(_) => {}
                Err(InsertError::DistanceTooLarge(_)) => {
                    self.nak_set.insert(segment.seq_no);
                }
                Err(InsertError::KeyTooLow(_)) => continue,
                Err(InsertError::WouldOverwrite(_)) => {
                    self.nak_set.insert(segment.seq_no);
                }
            }

            // Store the segment itself for later removal. If the buffer is full,
            // at it to the NAK set.
            match self.order_buf.push(segment) {
                Ok(_) => {}
                Err(InsertError::DistanceTooLarge(s)) => {
                    self.nak_set.insert(s.seq_no);
                }
                Err(InsertError::KeyTooLow(_)) => continue,
                Err(InsertError::WouldOverwrite(s)) => {
                    self.nak_set.insert(s.seq_no);
                }
            }
        }

        for segment in self.order_buf.drain() {
            self.nak_set.remove(&segment.seq_no);

            // Remove ACKed segments from out outgoing list
            self.outgoing_segments = self.outgoing_segments.take().map(|outgoing| {
                outgoing
                    .into_iter()
                    .filter(|(seq_no, _)| !segment.ack(*seq_no).is_ack())
                    .collect()
            });

            // Trigger immediate re-send for all outgoing NAKed segments
            self.outgoing_segments
                .as_mut()
                .unwrap()
                .iter_mut()
                .filter(|(seq_no, _)| segment.ack(**seq_no).is_nak())
                .for_each(|(_, outgoing)| outgoing.send_immediately());

            // TODO: This loses data if r_buf has too little space!
            let payloads = segment.chunks.into_iter().filter_map(|ch| match ch {
                Chunk::Payload(data) => Some(data),
                _ => None,
            });
            for payload in payloads {
                let copy_count = self.r_buf.remaining_mut().min(payload.len());
                self.r_buf.put_slice(&payload[..copy_count]);
            }
        }

        Ok(())
    }

    /// Asynchronously tries to write data to the stream.
    ///
    /// Note that until `.poll_flush()` is called, no data is actually written
    /// to the network.
    fn poll_write(&mut self, buf: &[u8]) -> Poll<usize, io::Error> {
        self.assert_can_write()?;

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
        self.assert_can_write()?;

        self.prepare_segments();
        self.poll_recv()?;

        // Gah I hate this style, but functional combinators don't allow
        // modifications to the control flow of this function and we need
        // those to handle erros and non-readyness.
        for outgoing in self.outgoing_segments.as_mut().unwrap().values_mut() {
            if !outgoing.should_send()? {
                continue;
            }
            if (self.remote_window_size as usize) < outgoing.data.len() {
                break;
            }

            try_ready!(self.send_socket.poll_send(&outgoing.data));

            outgoing.start_timers();
            self.remote_window_size
                .saturating_sub(outgoing.data.len() as u32);
        }

        self.poll_recv()?;
        Ok(Async::Ready(()))
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

            // Get a fresh sequence number and reduce our window size appropriately
            let seq_no = self.get_seq_no();
            let win_size = self.w_buf.remaining_mut();

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
                .seq_no(seq_no)
                .window_size(win_size as u32)
                .with_chunk(Chunk::Payload(payload.freeze()))
                .build();

            let serialized_segment = {
                self.segment_buf.reserve(segment.binary_len());
                segment
                    .write_to_with_crc32(&mut (&mut self.segment_buf).writer())
                    .unwrap(); // since we're writing to memory this is infallible

                self.segment_buf.split_to(self.segment_buf.len()).freeze()
            };

            self.outgoing_segments
                .as_mut()
                .unwrap()
                .insert(seq_no, Outgoing::new(serialized_segment));
        }

        // Since everything has been copied to segment_buf, this will reclaim
        // the entire buffer space without allocating.
        self.w_buf.reserve(BUF_SIZE);
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
        try_ready!(self.poll_flush());

        unimplemented!()
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

        return Ok(true);
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
