use crate::{
    accept::Accept,
    driver::{Driver, NEW_CONN_QUEUE_SIZE},
};
use futures::{
    prelude::*,
    sync::{mpsc, oneshot},
    try_ready,
};
use std::{io, net::SocketAddr};
use tokio::{self, net::udp::UdpSocket};

/// The panic message when polling after an I/O error has occured.
const POLL_AFTER_IO_ERR: &str = "cannot poll after an I/O error";

/// A stream of incoming SUTP connections.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct Incoming {
    listener: SutpListener,
}

/// An asynchronous SUTP connection listener.
#[derive(Debug)]
pub struct SutpListener {
    // This internally consists just of two channels that connect to the driver.
    // Both channels can be used asynchronously as not to block the event loop.
    /// A channel receiving newly opened connections.
    conn_recv: mpsc::Receiver<(Accept, SocketAddr)>,

    /// The driver to spawn on the first call to `.poll_accept()`.
    driver: Option<Driver>,

    /// A channel receiving hard I/O errors.
    io_err: oneshot::Receiver<io::Error>,
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
    type Item = (Accept, SocketAddr);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let conn = try_ready!(self.listener.poll_accept());

        Ok(Async::Ready(Some(conn)))
    }
}

impl SutpListener {
    /// Creates a socket bound to the given address and listens on it.
    pub fn bind(address: &SocketAddr) -> io::Result<Self> {
        UdpSocket::bind(address).map(Self::from_socket)
    }

    /// Binds the listener to the given UDP socket.
    pub fn from_socket(socket: UdpSocket) -> Self {
        let (io_err_tx, io_err_rx) = oneshot::channel();
        let (new_conn_tx, new_conn_rx) = mpsc::channel(NEW_CONN_QUEUE_SIZE);

        Self {
            conn_recv: new_conn_rx,
            driver: Some(Driver::new(socket, io_err_tx, new_conn_tx)),
            io_err: io_err_rx,
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
    pub fn poll_accept(&mut self) -> Poll<(Accept, SocketAddr), io::Error> {
        self.spawn_driver();
        self.poll_io_err()?;

        // Channel errors can only occur when the sender has been dropped, and
        // this only happens on hard I/O errors.
        match self
            .conn_recv
            .poll()
            .expect("mpsc::Receiver error")
        {
            // We're given IO errors as channel items
            Async::Ready(Some(conn)) => Ok(Async::Ready(conn)),
            Async::Ready(None) => panic!(POLL_AFTER_IO_ERR),
            Async::NotReady => Ok(Async::NotReady),
        }
    }

    /// Checks if the driver reported hard I/O errors.
    fn poll_io_err(&mut self) -> Result<(), io::Error> {
        match self.io_err.poll().expect(POLL_AFTER_IO_ERR) {
            Async::Ready(err) => Err(err),
            Async::NotReady => Ok(()),
        }
    }

    /// Spawns the associated driver, if present, onto the tokio runtime.
    fn spawn_driver(&mut self) {
        if let Some(d) = self.driver.take() {
            tokio::spawn(d);
        }
    }
}
