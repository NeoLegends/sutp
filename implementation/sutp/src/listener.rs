use bytes::BytesMut;
use futures::{
    future::Fuse,
    prelude::*,
    sync::mpsc::{channel, Receiver, Sender},
};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
};
use tokio::{
    self,
    net::udp::UdpSocket,
};

use segment::Segment;
use stream::SutpStream;

/// Max size of a UDP datagram.
const UDP_DGRAM_SIZE: usize = 1024 * 64;

/// A stream of incoming SUTP connections.
#[derive(Debug)]
pub struct Incoming {
    listener: SutpListener,
}

/// An SUTP listener.
#[derive(Debug)]
pub struct SutpListener {
    new_conn: Receiver<Result<(SutpStream, SocketAddr), io::Error>>,
}

/// The representation of a single SUTP connection from the driver's
/// point of view.
#[derive(Debug)]
struct Connection {
    segment_tx: Sender<Result<Segment, io::Error>>,
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
    conn_map: HashMap<SocketAddr, Connection>,
    new_conn: Option<Sender<Result<(SutpStream, SocketAddr), io::Error>>>,
    recv_buf: BytesMut,
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
    pub fn bind(address: &SocketAddr) -> io::Result<Self> {
        UdpSocket::bind(address)
            .map(Self::from_socket)
    }

    /// Binds the listener to the given UDP socket.
    pub fn from_socket(socket: UdpSocket) -> Self {
        let (tx, rx) = channel(128); // arbitrarily chosen, possibly make a parameter

        tokio::spawn(Driver::new(socket, tx));

        Self {
            new_conn: rx,
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
    /// Panics if not called within a future's execution context.
    pub fn poll_accept(&mut self) -> Poll<(SutpStream, SocketAddr), io::Error> {
        // Channel errors can only occur when the sender has been dropped, and this
        // only happens on hard I/O errors (think socket calls failed). All other
        // errors are passed through the channel to us here.
        match self.new_conn.poll().expect("cannot poll after an IO error") {
            // We're given IO errors as channel items
            Async::Ready(Some(maybe_conn)) => {
                match maybe_conn {
                    Ok(conn) => Ok(Async::Ready(conn)),
                    Err(e) => Err(e),
                }
            },
            _ => Ok(Async::NotReady),
        }
    }
}

impl Driver {
    /// Creates a new driver.
    pub fn new(
        socket: UdpSocket,
        new_conn: Sender<Result<(SutpStream, SocketAddr), io::Error>>,
    ) -> Fuse<Self> {
        Self {
            conn_map: HashMap::new(),
            new_conn: Some(new_conn),
            recv_buf: BytesMut::with_capacity(UDP_DGRAM_SIZE),
            socket: socket,
        }.fuse()
    }
}

impl Future for Driver {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if let Some(mut new_conn) = self.new_conn.take() {
            match new_conn.poll_complete() {
                Ok(Async::Ready(_)) => self.new_conn = Some(new_conn),
                Ok(Async::NotReady) => {
                    self.new_conn = Some(new_conn);
                    return Ok(Async::NotReady);
                },
                Err(_) => {},
            }
        }

        let (read, addr) = match self.socket.poll_recv_from(&mut self.recv_buf) {
            Ok(Async::Ready((n, addr))) => (n, addr),
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Err(e) => {
                //let _ = self.new_conn.start_send(Err(e));
                return Err(());
            },
        };

        unimplemented!()
    }
}
