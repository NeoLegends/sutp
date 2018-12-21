use futures::prelude::*;

use crate::branch::Branch;
use crate::graceful::Graceful;

/// Stream extensions.
pub trait StreamExt {
    /// Branches the stream into two, based on the given predicate function.
    ///
    /// The first branch receives all items for which `filter` returns true,
    /// the second branch receives all items for which `filter` returns false.
    ///
    /// Backpressure will propagate up, which means that one half will only
    /// receive its items as fast as the other half can process the others.
    ///
    /// After dropping one half, the remaining `Branch` behaves similar
    /// to `.filter()` discarding items it's not supposed to let through.
    fn branch<F>(
        self,
        filter: F,
    ) -> (Branch<Self, Self::Item, F>, Branch<Self, Self::Item, F>)
    where
        Self: Sized + Stream,
        F: FnMut(&Self::Item) -> bool;

    /// Converts the stream to a graceful stream, that, instead
    /// of passing on an error, passes on a Result of the item and the
    /// error in the Ok case.
    fn graceful(self) -> Graceful<Self>
    where
        Self: Sized;
}

impl<T: Stream> StreamExt for T {
    fn branch<F>(
        self,
        filter: F,
    ) -> (Branch<Self, T::Item, F>, Branch<T, T::Item, F>)
    where
        Self: Sized + Stream,
        F: FnMut(&T::Item) -> bool,
    {
        Branch::new(self, filter)
    }

    fn graceful(self) -> Graceful<Self>
    where
        Self: Sized,
    {
        Graceful::new(self)
    }
}
