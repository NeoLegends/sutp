// Macros need to lexically come before the rest to be usable
#[macro_use] mod macros;

mod chunk;
mod codec;
mod listener;
mod segment;
mod stream;

pub use crate::listener::{Incoming, SutpListener};
pub use crate::stream::{Connect, SutpStream};

/// Some useful extensions to `Result`.
trait ResultExt<T, E> {
    /// Allows mutable transformation on the value, without requiring the value
    /// to be returned.
    ///
    /// This is a shorthand for:
    ///
    /// ```rust
    /// # let result: Result<usize, ()> = Ok(5);
    /// # fn do_something(v: &mut usize) -> Result<usize, ()> { Ok(*v) }
    ///
    /// result.and_then(|mut v| {
    ///     do_something(&mut v)?;
    ///     Ok(v)
    /// });
    /// ```
    fn inspect_mut(self, f: impl FnOnce(&mut T) -> Result<(), E>) -> Result<T, E>;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn inspect_mut(self, f: impl FnOnce(&mut T) -> Result<(), E>) -> Self {
        self.and_then(|mut val| {
            f(&mut val)?;
            Ok(val)
        })
    }
}
