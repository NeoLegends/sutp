use bytes::BytesMut;
use futures::{
    prelude::*,
    sink::Send,
    sync::{mpsc, oneshot},
};
use std::{
    collections::HashMap,
    io::{self, Cursor},
    net::SocketAddr,
    u16,
};
use tokio::{
    self,
    net::udp::UdpSocket,
};

use ::ResultExt;
use segment::Segment;
use stream::{State, SutpStream};

/// Max size of a UDP datagram.
const UDP_DGRAM_SIZE: usize = u16::MAX as usize;

/// A stream of incoming SUTP connections.
#[derive(Debug)]
pub struct Incoming {
    listener: SutpListener,
}

/// An asynchronous SUTP connection listener.
#[derive(Debug)]
pub struct SutpListener {
    // This internally consists just of two channels that connect to the driver.
    // Both channels can be used asynchronously as not to block the event loop.

    /// A channel receiving hard I/O errors.
    io_err: oneshot::Receiver<io::Error>,

    /// A channel receiving newly opened connections.
    new_conn: mpsc::Receiver<(SutpStream, SocketAddr)>,
}

/// The background worker behind an SUTP listener.
///
/// Due to the fact that UDP sockets use a datagram-stealing-technique when used
/// with SO_REUSEPORT, we cannot create a separate UDP socket for each SUTP stream,
/// as the streams would steal each other's datagrams off the socket. This
/// neccesites using a shared socket with a broker multiplexing the received
/// datagrams to their destinations (i. e. the SUTP stream implementations).
///
/// This is spawned onto an executor when a listener is bound. It is responsible
/// for accepting new connections and distributing UDP datagrams to their target
/// SUTP streams for further processing.
///
/// It stops working once the listener and every SUTP stream has been dropped.
#[derive(Debug)]
struct Driver {
    /// A map of open connections.
    ///
    /// Maps from the remote address to a sender that's used to transmit the
    /// segment deserialization results into the corresponding SutpStream.
    ///
    /// When the corresponding receiver is dropped, the entry is removed
    /// from the map.
    conn_map: HashMap<SocketAddr, mpsc::Sender<Result<Segment, io::Error>>>,

    /// A oneshot channel to notify the listener about I/O failures.
    ///
    /// SutpStream's get notified about I/O errors only indirectly, by dropping
    /// the corresponding sender from the `conn_map`.
    io_err: Option<oneshot::Sender<io::Error>>,

    /// A channel used to transmit newly opened connections to the listener.
    new_conn: Option<mpsc::Sender<(SutpStream, SocketAddr)>>,

    /// The temporary future representing the new-connection-transmitting process.
    new_conn_fut: Option<Send<mpsc::Sender<(SutpStream, SocketAddr)>>>,

    /// A receive buffer for UDP data.
    recv_buf: BytesMut,

    /// The actual UDP socket.
    socket: UdpSocket,
}

impl Incoming {
    /// Gets a mutable reference to the underlying SUTP listener.
    pub fn get_mut(&mut self) -> &mut SutpListener {
        &mut self.listener
    }

    /// Gets a reference to the underlying SUTP listener.
    pub fn get_ref(&self) -> &SutpListener {
        &self.listener
    }

    /// Gets the SUTP listener back out of the stream.
    pub fn into_inner(self) -> SutpListener {
        self.listener
    }
}

impl Stream for Incoming {
    type Item = (SutpStream, SocketAddr);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let conn = try_ready!(self.listener.poll_accept());

        Ok(Async::Ready(Some(conn)))
    }
}

impl SutpListener {
    /// Creates a socket bound to the given address and listens on it.
    ///
    /// ## Panics
    ///
    /// Panics if the listener driver cannot be spawned onto the default executor.
    pub fn bind(address: &SocketAddr) -> io::Result<Self> {
        UdpSocket::bind(address)
            .map(Self::from_socket)
    }

    /// Binds the listener to the given UDP socket.
    ///
    /// ## Panics
    ///
    /// Panics if the listener driver cannot be spawned onto the default executor.
    pub fn from_socket(socket: UdpSocket) -> Self {
        let (io_err_tx, io_err_rx) = oneshot::channel();
        // 4 is arbitrarily chosen, possibly make a parameter
        let (new_conn_tx, new_conn_rx) = mpsc::channel(4);

        tokio::spawn(Driver::new(socket, io_err_tx, new_conn_tx));

        Self {
            io_err: io_err_rx,
            new_conn: new_conn_rx,
        }
    }

    /// Converts this listener into a stream of incoming connections.
    pub fn incoming(self) -> Incoming {
        Incoming { listener: self }
    }

    /// Asynchronously accepts a new connection. On success, returns the
    /// connected stream and the remote address.
    ///
    /// This method will silently discard any received UDP segments that
    /// aren't proper SUTP segments or are invalid.
    ///
    /// ## Panics
    ///
    /// Panics if not called within a future's execution context or when
    /// polling after an I/O error.
    pub fn poll_accept(&mut self) -> Poll<(SutpStream, SocketAddr), io::Error> {
        // Check if there were I/O errors before querying the connection channel
        match self.io_err.poll() {
            Ok(Async::Ready(err)) => return Err(err),
            Ok(Async::NotReady) => {},
            Err(_) => panic!("driver has gone away"),
        }

        // Channel errors can only occur when the sender has been dropped, and
        // this only happens on hard I/O errors.
        match self.new_conn.poll().expect("cannot poll after an IO error") {
            // We're given IO errors as channel items
            Async::Ready(Some(conn)) => Ok(Async::Ready(conn)),
            _ => Ok(Async::NotReady),
        }
    }
}

impl Driver {
    /// Creates a new driver.
    pub fn new(
        socket: UdpSocket,
        io_err: oneshot::Sender<io::Error>,
        new_conn: mpsc::Sender<(SutpStream, SocketAddr)>,
    ) -> Self {
        Self {
            conn_map: HashMap::new(),
            io_err: Some(io_err),
            new_conn: Some(new_conn),
            new_conn_fut: None,
            recv_buf: BytesMut::with_capacity(UDP_DGRAM_SIZE),
            socket: socket,
        }
    }

    /// Asynchronously reads a segment off the UDP socket.
    ///
    /// The segment is also validated.
    fn recv_segment(&mut self)
        -> Poll<(Result<Segment, io::Error>, SocketAddr), io::Error> {
        self.recv_buf.reserve(UDP_DGRAM_SIZE);

        let (nread, addr) = try_ready!({
            self.socket.poll_recv_from(&mut self.recv_buf)
        });

        let mut buf = self.recv_buf.split_to(nread);
        let mut buf = Cursor::new(buf.as_mut());

        let maybe_segment = Segment::read_from_and_validate(&mut buf);
        Ok(Async::Ready((maybe_segment, addr)))
    }
}

/// DRY-macro for hard I/O errors within the driver.
///
/// Attempts to send the error via the contained io_err channel
/// to the listener and shuts down the driver.
macro_rules! hard_io_err {
    ($this:ident, $err:expr) => {{
        let _ = $this.io_err.take()
            .expect("polling after I/O error")
            .send($err);

        // Shutdown the driver by completing the future
        return Ok(Async::Ready(()));
    }};
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
            if self.new_conn.is_none() &&
                self.new_conn_fut.is_none() &&
                self.conn_map.is_empty() {
                return Ok(Async::Ready(()));
            }

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
                    Err(_) => {},
                }
            }
            self.new_conn_fut = None;

            // Read a segment from the socket
            let (maybe_segment, addr) = match self.recv_segment() {
                Ok(Async::Ready(data)) => data,
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(e) => hard_io_err!(self, e),
            };

            // Currently needed due to borrowing issues, I hope NLL will
            // make these variables obsolete soon.
            let mut conn_dead = false;
            let mut new_conn_tx = None;

            if let Some(conn) = self.conn_map.get_mut(&addr) {
                // We have a connection which we need to forward the segment
                // parsing result to.

                // TODO: We'd actually would like to apply backpressure here, but
                // not sure how without affecting all other connections as well.
                match conn.try_send(maybe_segment) {
                    Ok(_) => {},
                    Err(ref e) if e.is_disconnected() => conn_dead = true,
                    Err(ref e) if e.is_full() => {
                        warn!("discarding segment due to overpressure");
                    },
                    Err(e) => unreachable!("unknown channel failure: {:?}", e),
                }
            } else if let Some(new_conn) = self.new_conn.take() {
                // We need to check whether the given segment is a SYN-> segment
                // and actually initialize the new connection.

                if maybe_segment.is_err() {
                    trace!("discarding segment because its invalid");
                    self.new_conn = Some(new_conn);

                    continue;
                }

                // Safe due to check above
                let segment = maybe_segment.unwrap();

                if !segment.is_syn1() {
                    trace!("discarding init segment because it's not SYN->");
                    self.new_conn = Some(new_conn);

                    continue;
                }

                // Create sending socket and bind it to the remote address
                let maybe_sock = UdpSocket::bind(&addr)
                    .inspect_mut(|s| s.connect(&addr));
                let sock = match maybe_sock {
                    Ok(sock) => sock,
                    Err(e) => hard_io_err!(self, e),
                };

                // TODO: The 8 here is chosen arbitrarily, but right now this
                // very much affects our performance. If this channel overflows,
                // we don't apply backpressure right now, instead we just discard
                // the segments and don't even notify the sender about that.
                let (mut tx, rx) = mpsc::channel(8);

                // Queue initial segment for processing in the SutpStream
                tx.try_send(Ok(segment)).expect("failed to queue initial segment");

                let stream = SutpStream::from_listener(sock, rx, State::SynRcvd);

                new_conn_tx = Some(tx);
                self.new_conn_fut = Some(new_conn.send((stream, addr.clone())));
            } else {
                // The segment is invalid and we don't know where it's coming from,
                // or the listener has been dropped and we cannot accept new
                // connections.

                trace!("received invalid segment from unknown address");
            }

            if conn_dead {
                self.conn_map.remove(&addr);
            }
            if let Some(tx) = new_conn_tx {
                self.conn_map.insert(addr, tx);
            }
        }
    }
}
