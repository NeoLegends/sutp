use futures::{
    prelude::*,
    sink::Send,
    sync::{
        BiLock,
        mpsc::{channel, Receiver, Sender},
    },
};

/// A stream similar to `.filter()` that splits up a stream into two based
/// on a predicate.
///
/// Backpressure will propagate up, which means that one half will only receive
/// its items as fast as the other half can process the others.
#[derive(Debug)]
pub struct Branch<T, I, F> {
    filter_val: bool,
    inner: BiLock<Inner<T, F>>,
    recv: Receiver<I>,
    send: Option<Sender<I>>,
    send_fut: Option<Send<Sender<I>>>,
}

#[derive(Debug)]
struct Inner<T, F> {
    pub filter: F,
    pub stream: T,
}

impl<T: Stream, F: FnMut(&T::Item) -> bool> Branch<T, T::Item, F> {
    /// Branches the given stream into two, based on the given predicate.
    ///
    /// The first branch receives the items where `filter` returns true,
    /// the other one receives the items where `filter` returns false.
    ///
    /// Errors are passed to the stream which ever is currently `poll`ing.
    pub fn new(
        stream: T,
        filter: F,
    ) -> (Branch<T, T::Item, F>, Branch<T, T::Item, F>) {
        // Create rendezvous channels for backpressure
        let (tx_a, rx_b) = channel(0);
        let (tx_b, rx_a) = channel(0);

        let (inner_a, inner_b) = BiLock::new(
            Inner { filter, stream }
        );

        let a = Branch {
            filter_val: true,
            inner: inner_a,
            recv: rx_a,
            send: Some(tx_a),
            send_fut: None,
        };
        let b = Branch {
            filter_val: false,
            inner: inner_b,
            recv: rx_b,
            send: Some(tx_b),
            send_fut: None,
        };

        (a, b)
    }
}

impl<T: Stream, F: FnMut(&T::Item) -> bool> Stream for Branch<T, T::Item, F> {
    type Item = T::Item;
    type Error = T::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            // If we're currently pushing an item to the other half, see
            // if that has completed.
            if let Some(mut fut) = self.send_fut.take() {
                match fut.poll() {
                    Ok(Async::Ready(sender)) => self.send = Some(sender),
                    Ok(Async::NotReady) => {
                        self.send_fut = Some(fut);
                        return Ok(Async::NotReady);
                    },

                    // The receiver has been dropped, this means the other half
                    // has been dropped. We don't really care about this, since
                    // the user probably wants it to be like this.
                    Err(_) => {},
                }
            }

            // Check if the other task has pushed items into our queue
            match self.recv.poll() {
                Ok(Async::Ready(Some(it))) => return Ok(Async::Ready(Some(it))),
                Ok(Async::Ready(None)) | Ok(Async::NotReady) => {},
                Err(_) => panic!("receiver cannot error"),
            };

            // Now acquire the lock and poll the underlying stream for items.
            let (filter_val, item) = {
                let mut lock = try_ready!(Ok(self.inner.poll_lock()));

                match try_ready!(lock.stream.poll()) {
                    Some(it) => ((&mut lock.filter)(&it), it),
                    None => return Ok(Async::Ready(None)),
                }
            };

            // If the item is destined for us, return it. Otherwise push it
            // over to the other half (if it still exists) and drive that to
            // completion.
            if filter_val == self.filter_val {
                return Ok(Async::Ready(Some(item)));
            } else {
                self.send_fut = self.send.take()
                    .map(|ch| ch.send(item));
            }
        }
    }
}
