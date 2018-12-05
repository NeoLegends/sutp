use futures::prelude::*;

/// A stream that converts the errors of the underlying stream into
/// Ok(Result<T, E>) cases.
///
/// Graceful<T> is guaranteed never to error.
#[derive(Debug)]
pub struct Graceful<T> {
    stream: T,
}

impl<T> Graceful<T> {
    pub fn new(stream: T) -> Self {
        Graceful { stream }
    }
}

impl<T: Stream> Stream for Graceful<T> {
    type Item = Result<T::Item, T::Error>;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.stream.poll() {
            Ok(Async::Ready(Some(it))) => Ok(Async::Ready(Some(Ok(it)))),
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => Ok(Async::Ready(Some(Err(e)))),
        }
    }
}
