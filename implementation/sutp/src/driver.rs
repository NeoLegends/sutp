//! Implements the background processor.

use crate::{accept::Accept, segment::Segment, RECEIVER_ERROR, UDP_DGRAM_SIZE};
use bytes::{Bytes, BytesMut};
use futures::{
    prelude::*,
    sink::Send,
    sync::{mpsc, oneshot},
    try_ready,
};
use log::{trace, warn};
use std::{collections::HashMap, io, net::SocketAddr};
use tokio::{self, net::udp::UdpSocket};

/// DRY-macro for hard I/O errors within the driver.
///
/// Attempts to send the error via the contained io_err channel
/// to the listener and shuts down the driver.
macro_rules! hard_io_err {
    ($this:ident, $err:expr) => {{
        let _ = $this
            .io_err
            .take()
            .expect("polling after I/O error")
            .send($err);

        // Shutdown the driver by completing the future
        return Err(());
    }};
}

/// The size of the queue for new connections.
pub const NEW_CONN_QUEUE_SIZE: usize = 8;

// TODO: The 8 here is chosen arbitrarily, but right now this
// very much affects our performance. If this channel overflows,
// we don't apply backpressure right now, instead we just discard
// the segments and don't even notify the sender about that.

/// The size of a channel for newly arriving segments.
const STREAM_SEGMENT_QUEUE_SIZE: usize = 8;

/// The background worker behind SUTP listeners and streams.
///
/// Due to the fact that UDP sockets use a datagram-stealing-technique when used
/// with SO_REUSEPORT, we cannot create a separate UDP socket for each SUTP stream,
/// as the streams would steal each other's datagrams off the socket. This
/// neccesites using a shared socket with a broker multiplexing the received
/// datagrams to their destinations (i. e. the SUTP stream implementations).
///
/// This is spawned onto an executor when a listener is bound. It is responsible
/// for accepting new connections and distributing UDP datagrams to their target
/// SUTP streams for further processing as well as sending out any segments over
/// the shared UDP socket.
///
/// It stops working once the listener and every connected SUTP stream has been
/// dropped.
#[derive(Debug)]
pub struct Driver {
    /// A map of open connections.
    ///
    /// Maps from the remote address to a sender that's used to transmit the
    /// segment deserialization results into the corresponding SutpStream.
    ///
    /// When the corresponding receiver is dropped, the entry is removed
    /// from the map.
    conn_map: HashMap<SocketAddr, mpsc::Sender<Result<Segment, io::Error>>>,

    /// The segment that is currently being sent over the UDP socket.
    ///
    /// This is necessary because when receiving a segment from the sending channel,
    /// the UDP socket may not be ready for sending it out.
    currently_sending: Option<(Bytes, SocketAddr)>,

    /// A oneshot channel to notify the listener about I/O failures.
    ///
    /// SutpStream's get notified about I/O errors only indirectly, by dropping
    /// the corresponding sender from the `conn_map`.
    io_err: Option<oneshot::Sender<io::Error>>,

    /// A channel used to transmit newly opened connections to the listener.
    new_conn: Option<mpsc::Sender<(Accept, SocketAddr)>>,

    /// The temporary future representing the new-connection-transmitting process.
    new_conn_fut: Option<Send<mpsc::Sender<(Accept, SocketAddr)>>>,

    /// A receive buffer for UDP data.
    recv_buf: BytesMut,

    /// The channel to receive segments to send over.
    segment_rx: mpsc::Receiver<(Bytes, SocketAddr)>,

    /// The sending side of the `segment_rx`, kept here for cloning it for new
    /// connections.
    ///
    /// If this is `None`, new connections cannot be accepted anymore.
    segment_tx: Option<mpsc::Sender<(Bytes, SocketAddr)>>,

    /// The channel any existing streams publish their dropping to.
    ///
    /// This is used to properly stop the driver when streams have dropped.
    shutdown_rx: mpsc::UnboundedReceiver<SocketAddr>,

    /// The sending side of the `shutdown_tx`, kept here for cloning it for new
    /// connections.
    ///
    /// If this is `None`, new connections cannot be accepted anymore.
    shutdown_tx: Option<mpsc::UnboundedSender<SocketAddr>>,

    /// The actual UDP socket.
    socket: UdpSocket,
}

impl Driver {
    /// Creates a new driver that listens for and drives new connections.
    pub fn new(
        socket: UdpSocket,
        io_err: oneshot::Sender<io::Error>,
        new_conn: mpsc::Sender<(Accept, SocketAddr)>,
    ) -> Self {
        let (sgmt_tx, sgmt_rx) = mpsc::channel(NEW_CONN_QUEUE_SIZE);
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded();

        Self {
            conn_map: HashMap::new(),
            currently_sending: None,
            io_err: Some(io_err),
            new_conn: Some(new_conn),
            new_conn_fut: None,
            recv_buf: BytesMut::with_capacity(UDP_DGRAM_SIZE),
            segment_rx: sgmt_rx,
            segment_tx: Some(sgmt_tx),
            shutdown_rx,
            shutdown_tx: Some(shutdown_tx),
            socket,
        }
    }

    /// Creates a new driver driving the given connection.
    ///
    /// The driver will not accept new connections.
    pub fn from_connection(
        socket: UdpSocket,
        io_err: oneshot::Sender<io::Error>,
        addr: &SocketAddr,
        connection_tx: mpsc::Sender<Result<Segment, io::Error>>,
        segment_rx: mpsc::Receiver<(Bytes, SocketAddr)>,
        shutdown_rx: mpsc::UnboundedReceiver<SocketAddr>,
    ) -> Self {
        let conn_map = {
            let mut map = HashMap::with_capacity(1);
            map.insert(*addr, connection_tx);
            map
        };

        Self {
            conn_map,
            currently_sending: None,
            io_err: Some(io_err),
            new_conn: None,
            new_conn_fut: None,
            recv_buf: BytesMut::with_capacity(UDP_DGRAM_SIZE),
            segment_rx,
            segment_tx: None,
            shutdown_rx,
            shutdown_tx: None,
            socket,
        }
    }

    /// Processes newly arriving segments on the UDP socket.
    fn poll_recv(&mut self) -> Poll<(), ()> {
        // Ensure new connections are getting picked up
        //
        // TODO: Refactor this somehow. This is super less-than-ideal,
        // because receiving a lot of new connections can block the processing
        // of already-opened ones. Ideally we'd separate these concerns and let
        // the backpressure of new connections not apply to the handling of
        // existing connections.
        if let Some(ref mut fut) = self.new_conn_fut.as_mut() {
            match fut.poll() {
                Ok(Async::Ready(sender)) => self.new_conn = Some(sender),
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                // Listener has been dropped, but some streams may still be alive
                Err(_) => {}
            }
        }
        self.new_conn_fut = None;

        // Read a segment from the socket
        let (maybe_segment, addr) = match self.recv_udp_segment() {
            Ok(Async::Ready(data)) => data,
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Err(e) => hard_io_err!(self, e),
        };

        if let Some(conn) = self.conn_map.get_mut(&addr) {
            trace!("serving existing conn from {}", addr);

            // We have a connection which we need to forward the segment
            // parsing result to.

            // TODO: We'd actually would like to apply backpressure here, but
            // not sure how without affecting all other connections as well.
            match conn.try_send(maybe_segment) {
                Ok(_) => {}
                Err(ref e) if e.is_disconnected() => {
                    trace!("removing disconnected stream to {}", addr);
                    self.conn_map.remove(&addr);
                }
                Err(ref e) if e.is_full() => {
                    warn!("discarding segment due to overpressure");
                }
                Err(e) => unreachable!("unknown channel failure: {:?}", e),
            }
        } else if let Some(new_conn) = self.new_conn.take() {
            trace!("setting up new conn to {}", addr);

            // We need to check whether the given segment is a SYN-> segment
            // and actually initialize the new connection.

            let segment = match maybe_segment {
                Ok(segment) => segment,
                Err(e) => {
                    trace!("discarding invalid segment due to {:?}", e);
                    self.new_conn = Some(new_conn);

                    return Ok(Async::Ready(()));
                }
            };

            if !segment.is_syn1() {
                trace!("discarding init segment because it's not SYN->");
                self.new_conn = Some(new_conn);

                return Ok(Async::Ready(()));
            }

            trace!("setting up accept future");

            let (segment_tx, from_driver_tx, from_driver_rx, shutdown_tx) =
                self.prepare_channels(addr, segment);
            let accept = Accept::from_listener(
                addr,
                from_driver_rx,
                from_driver_tx,
                segment_tx,
                shutdown_tx,
            );

            self.new_conn_fut = Some(new_conn.send((accept, addr)));
        } else {
            // The segment is invalid and we don't know where it's coming from,
            // or the listener has been dropped and we cannot accept new
            // connections.

            trace!("received invalid segment from unknown address");
        }

        Ok(Async::Ready(()))
    }

    /// Sends a single segment, if possible, over the wire.
    fn poll_send(&mut self) -> Poll<(), ()> {
        // Store the channel result in the driver to care for the case where we
        // receive an item over the channel but the socket isn't ready to send.
        let (data, addr) = if let Some(data) = self.currently_sending.as_ref() {
            data
        } else {
            let outgoing = match self.segment_rx.poll() {
                Ok(Async::Ready(Some(item))) => item,
                Ok(Async::Ready(None)) | Ok(Async::NotReady) => {
                    return Ok(Async::NotReady)
                }
                Err(_) => unreachable!(), // Receiver::poll() never errors
            };

            self.currently_sending = Some(outgoing);
            self.currently_sending.as_ref().unwrap()
        };

        match self.socket.poll_send_to(data, addr) {
            Ok(Async::Ready(_)) => {}
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Err(e) => hard_io_err!(self, e),
        }

        self.currently_sending = None;
        Ok(Async::Ready(()))
    }

    /// Polls for any streams that have shut down to remove them from the
    /// connection table.
    fn poll_shutdown(&mut self) -> Poll<(), ()> {
        loop {
            match self.shutdown_rx.poll().expect(RECEIVER_ERROR) {
                Async::Ready(Some(addr)) => {
                    trace!("removing shut down connection to {}", addr);
                    self.conn_map.remove(&addr);
                }
                _ => return Ok(Async::NotReady),
            };
        }
    }

    /// Sets up the necessary channels from and to the driver for a new connection
    /// to `addr` with the initial segment `init_sgmt`.
    #[allow(clippy::type_complexity)]
    fn prepare_channels(
        &mut self,
        address: SocketAddr,
        init_sgmt: Segment,
    ) -> (
        mpsc::Sender<(Bytes, SocketAddr)>,
        mpsc::Sender<Result<Segment, io::Error>>,
        mpsc::Receiver<Result<Segment, io::Error>>,
        mpsc::UnboundedSender<SocketAddr>,
    ) {
        let (mut from_driver_tx, from_driver_rx) =
            mpsc::channel(STREAM_SEGMENT_QUEUE_SIZE);

        // Queue initial segment for processing in the accept future.
        // This doesn't fail, only when the allocation fails.
        from_driver_tx.try_send(Ok(init_sgmt)).unwrap();

        self.conn_map.insert(address, from_driver_tx.clone());

        let segment_tx = self
            .segment_tx
            .as_ref()
            .cloned()
            .expect("missing segment tx");
        let shutdown_tx = self
            .shutdown_tx
            .as_ref()
            .cloned()
            .expect("missing segment tx");

        (segment_tx, from_driver_tx, from_driver_rx, shutdown_tx)
    }

    /// Asynchronously reads a segment off the UDP socket.
    ///
    /// The segment is also validated.
    fn recv_udp_segment(
        &mut self,
    ) -> Poll<(Result<Segment, io::Error>, SocketAddr), io::Error> {
        self.recv_buf.reserve(UDP_DGRAM_SIZE);

        // Unsafe because we're setting the length without any checks here. This
        // may potentially result in reads from uninitialized memory, but the UDP
        // socket will not attempt to read from the buffer, so it's not a big deal
        let (nread, addr) = unsafe {
            self.recv_buf.set_len(UDP_DGRAM_SIZE);
            let (nread, addr) =
                try_ready!(self.socket.poll_recv_from(self.recv_buf.as_mut()));
            self.recv_buf.set_len(nread);

            (nread, addr)
        };

        let mut buf = self.recv_buf.split_to(nread).freeze();
        let maybe_segment = Segment::read_from_and_validate(&mut buf);
        Ok(Async::Ready((maybe_segment, addr)))
    }

    /// Determines whether the driver can still perform work or should quit.
    fn should_quit(&self) -> bool {
        self.new_conn.is_none()
            && self.new_conn_fut.is_none()
            && self.conn_map.is_empty()
    }
}

// To make the driver work in the background on an executor, we represent
// it as future that only ever resolves in case of an error, or when the
// listener and all streams have been dropped.

impl Future for Driver {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            // If the new_conn channel is closed and the connection map is empty
            // we need to shut down, because everything has been dropped and
            // there's nothing to forward data to.
            if self.should_quit() {
                trace!("driver shutting down");
                return Ok(Async::Ready(()));
            }

            // Poll until everything returns NotReady
            if let (Async::NotReady, Async::NotReady, Async::NotReady) =
                (self.poll_shutdown()?, self.poll_recv()?, self.poll_send()?)
            {
                return Ok(Async::NotReady);
            }
        }
    }
}
