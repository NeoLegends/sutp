use std::{
    error::Error,
    fmt::{Debug, Display, Formatter, Result as FmtResult},
};

/// A sparse, sorted ring-like buffer using a key function.
///
/// This buffer represents a sorted ring of slots that can be either filled or
/// empty. When an element is inserted, its position is computed using an external
/// comparator function. The function is allowed to return values of unbounded
/// size, since the sparse buffer will make the indices relative to the lowest
/// stored value.
///
/// Values can only be removed from the "start" of the buffer in ascending position.
/// Additionally, values can only be removed until the first "hole" is reached.
/// The hole needs to be filled before access to the other values is possible.
#[derive(Clone, Eq, PartialEq)]
pub struct SparseBuffer<T, F> {
    buf: Vec<Option<T>>,
    head: usize,
    key_fn: F,
    lowest_key: Option<usize>,
}

/// An iterator that repeatedly calls `.pop()` on the underlying sparse buffer.
#[derive(Debug, Eq, PartialEq)]
pub struct Drain<'a, T, F> {
    buf: &'a mut SparseBuffer<T, F>,
}

/// The error that can occur while inserting a value into the sparse buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InsertError<T> {
    /// The element cannot be inserted because the distance to the other elements
    /// is too large and the underlying buffer doesn't have the capacity to store
    /// that.
    DistanceTooLarge(T),

    /// The element cannot be inserted because the key as given by the key function
    /// would place the element before the current head. This is a violation of the
    /// invariants of the sparse buffer.
    ///
    /// This can only occur if there are items in the sparse buffer. Inserting into
    /// an empty buffer will never throw this kind of error.
    KeyTooLow(T),

    /// The element cannot be inserted because another element would be overwritten.
    WouldOverwrite(T),
}

impl<T, F> SparseBuffer<T, F> {
    /// Creates a new sparse buffer.
    ///
    /// Care must be taken that `key_fn` yields monotonically increasing keys.
    ///
    /// # Panics
    ///
    /// Panics if the given capacity is 0.
    pub fn new(capacity: usize, key_fn: F) -> Self {
        assert!(capacity > 0);

        // Can't use the vec![None, capacity]-macro here, because while `None`
        // itself would be copyable, `Option<T>` is not, and that's what the
        // compiler sees.
        let mut vec = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            vec.push(None);
        }

        Self {
            buf: vec,
            head: 0,
            key_fn,
            lowest_key: None,
        }
    }

    /// Returns the amount of elements that can be `.pop()`ed without
    /// receiving `None`.
    pub fn available(&self) -> usize {
        self.buf
            .iter()
            .cycle() // Ensure we wrap around at the end
            .skip(self.head) // Skip to the head
            .take(self.capacity()) // Ensure we're bounded
            .take_while(|slot| slot.is_some()) // and count the full slots
            .count()
    }

    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// The amount of slots filled.
    ///
    /// Note that the elements aren't necessarily `.pop()`able due to possible
    /// holes in the buffer.
    pub fn count(&self) -> usize {
        self.buf.iter().filter(|slot| slot.is_some()).count()
    }

    /// Resets the sparse buffer to its initial state.
    pub fn clear(&mut self) {
        for it in &mut self.buf {
            *it = None;
        }

        self.lowest_key = None;
        self.head = 0;
    }

    /// Checks whether the sparse buffer is empty.
    pub fn is_empty(&self) -> bool {
        // If we have an element, the lowest key is set. We can "abuse" this
        // to check whether the buffer is empty.
        self.lowest_key.is_none()
    }
}

impl<T, F: Fn(&T) -> usize> SparseBuffer<T, F> {
    /// Obtains a draining iterator that repeatedly calls `.pop()`.
    pub fn drain(&mut self) -> Drain<'_, T, F> {
        Drain { buf: self }
    }

    /// Gets the key of the highest element in the consecutive row from the
    /// current head without removing any of the elements.
    pub fn highest_consecutive_key(&self) -> Option<usize> {
        let mut last_some_slot = None;
        self.buf
            .iter()
            .cycle() // Ensure we wrap around at the end
            .skip(self.head) // Skip to the head
            .take(self.capacity()) // Ensure we're bounded
            .skip_while(|slot| {
                if let Some(slot_val) = slot {
                    last_some_slot = Some(slot_val);
                    true
                } else {
                    false
                }
            })
            .next();

        last_some_slot.map(|slot_val| (self.key_fn)(slot_val))
    }

    /// Returns a reference to the smallest element in the sparse buffer without
    /// removing it from the buffer.
    pub fn peek(&self) -> Option<&T> {
        self.buf[self.head].as_ref()
    }

    /// Attempts to remove the "smallest" item from the sparse buffer.
    ///
    /// Returns `None` if the buffer is empty or if there is a hole at the current
    /// position.
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }

        // Take out the element
        let element = self.buf[self.head].take();

        // Advance the head if we have actually removed something from the
        // vector and don't skip past holes.
        self.head = if element.is_some() {
            (self.head + 1) % self.capacity()
        } else {
            self.head
        };

        // If we didn't remove an element, the lowest key stays the same. Otherwise
        // reset it if we're empty and increment it by one (to make it "point" to
        // the next slot), if not.
        self.lowest_key = if element.is_none() {
            self.lowest_key
        } else if self.buf.iter().all(|slot| slot.is_none()) {
            // Note: Can't use self.is_empty() here because we're recomputing the
            // value is_empty() relies on.
            //
            // TODO: Can we get this down to `O(1)`?

            None
        } else {
            Some(self.lowest_key.unwrap().wrapping_add(1))
        };

        element
    }

    /// Inserts an element into the sparse buffer.
    ///
    /// This operation is `O(1)`.
    pub fn push(&mut self, val: T) -> Result<(), InsertError<T>> {
        let key = (self.key_fn)(&val);

        // TODO: What if `key` wraps around after reaching the max value?

        let insert_pos = if !self.is_empty() {
            // It is guaranteed that either the sparse buffer is empty or that
            // the currently lowest key is stored within the buffer.

            let checked_distance = key.checked_sub(self.lowest_key.unwrap());

            // Ensure the distance stays within valid bounds.
            //
            // dist >= self.capacity() implies the distance is too large, dist < 0
            // implies that the key is too low and the element would be inserted
            // before the current head (which not supported).
            let distance_from_lowest = match checked_distance {
                Some(val) if val < self.capacity() => val,
                Some(_) => return Err(InsertError::DistanceTooLarge(val)),
                None => return Err(InsertError::KeyTooLow(val)),
            };

            distance_from_lowest.wrapping_add(self.head) % self.capacity()
        } else {
            0
        };

        if self.buf[insert_pos].is_some() {
            return Err(InsertError::WouldOverwrite(val));
        }
        if self.is_empty() {
            self.head = insert_pos;
            self.lowest_key = Some(key);
        }

        self.buf[insert_pos] = Some(val);
        Ok(())
    }
}

impl<T: Debug, F> Debug for SparseBuffer<T, F> {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        f.debug_struct(stringify!(SparseBuffer<T, F>))
            .field("buf", &self.buf)
            .field("head", &self.head)
            .field("key_fn", &"...")
            .field("lowest_key", &self.lowest_key)
            .finish()
    }
}

impl<'a, T, F: Fn(&T) -> usize> Iterator for Drain<'a, T, F> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.buf.pop()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let available = self.buf.available();
        (available, Some(available))
    }
}

impl<'a, T, F: Fn(&T) -> usize> ExactSizeIterator for Drain<'a, T, F> {}

impl<T> InsertError<T> {
    /// Checks if the error represents the "distance too large" variant.
    pub fn is_distance_too_large(&self) -> bool {
        match self {
            InsertError::DistanceTooLarge(_) => true,
            _ => false,
        }
    }

    /// Checks if the error represents the "key too low" variant.
    pub fn is_key_too_low(&self) -> bool {
        match self {
            InsertError::KeyTooLow(_) => true,
            _ => false,
        }
    }

    /// Checks if the error represents the "would overwrite" variant.
    pub fn is_would_overwrite(&self) -> bool {
        match self {
            InsertError::WouldOverwrite(_) => true,
            _ => false,
        }
    }
}

impl<T> Display for InsertError<T> {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        match self {
            InsertError::DistanceTooLarge(_) => write!(
                fmt,
                "cannot insert element, the distance to the other elements is too large",
            ),
            InsertError::KeyTooLow(_) => {
                write!(fmt, "cannot insert element because the key is too low",)
            }
            InsertError::WouldOverwrite(_) => write!(
                fmt,
                "cannot insert element because it would overwrite another element",
            ),
        }
    }
}

impl<T: Debug> Error for InsertError<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        assert!(buf.is_empty());

        assert_eq!({ buf.drain().size_hint() }, (0, Some(0)));

        buf.push(3).unwrap();
        assert_eq!({ buf.drain().size_hint() }, (1, Some(1)));

        buf.push(4).unwrap();
        assert_eq!({ buf.drain().size_hint() }, (2, Some(2)));

        buf.push(5).unwrap();
        assert_eq!({ buf.drain().size_hint() }, (3, Some(3)));

        buf.pop().unwrap();
        assert_eq!({ buf.drain().size_hint() }, (2, Some(2)));

        buf.push(6).unwrap();
        assert_eq!({ buf.drain().size_hint() }, (3, Some(3)));
    }

    #[test]
    fn smoke() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        assert!(buf.is_empty());

        buf.push(3).unwrap();
        buf.push(4).unwrap();
        buf.push(5).unwrap();

        assert!(!buf.is_empty());

        assert_eq!(buf.pop(), Some(3));
        assert_eq!(buf.pop(), Some(4));
        assert_eq!(buf.pop(), Some(5));
        assert_eq!(buf.pop(), None);
        assert_eq!(buf.pop(), None);
    }

    #[test]
    fn cannot_overwrite() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        buf.push(3).unwrap();
        match buf.push(3) {
            Ok(_) => panic!("didn't get error"),
            Err(InsertError::WouldOverwrite(_)) => {}
            Err(_) => panic!("did get wrong error"),
        }
    }

    #[test]
    fn distance_too_large() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        buf.push(3).unwrap();
        match buf.push(100) {
            Ok(_) => panic!("didn't get error"),
            Err(InsertError::DistanceTooLarge(_)) => {}
            Err(_) => panic!("did get wrong error"),
        }
    }

    #[test]
    fn drain_hole() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        assert!(buf.is_empty());

        buf.push(3).unwrap();
        buf.push(5).unwrap();

        assert!(!buf.is_empty());

        assert_eq!(buf.pop(), Some(3));
        assert_eq!(buf.pop(), None);
        assert_eq!(buf.pop(), None);

        buf.push(4).unwrap();

        assert_eq!(buf.pop(), Some(4));
        assert_eq!(buf.pop(), Some(5));
        assert_eq!(buf.pop(), None);
        assert_eq!(buf.pop(), None);
    }

    #[test]
    fn drain_and_fill() {
        let mut buf = SparseBuffer::new(5, |v: &usize| *v);

        for i in 0..100 {
            assert!(buf.is_empty());

            buf.push(i * 3 + 3).unwrap();
            buf.push(i * 3 + 4).unwrap();
            buf.push(i * 3 + 5).unwrap();

            assert!(!buf.is_empty());

            assert_eq!(buf.pop(), Some(i * 3 + 3));
            assert_eq!(buf.pop(), Some(i * 3 + 4));
            assert_eq!(buf.pop(), Some(i * 3 + 5));
            assert_eq!(buf.pop(), None);
            assert_eq!(buf.pop(), None);
        }
    }

    #[test]
    fn highest_consecutive_key() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        buf.push(3).unwrap();
        assert_eq!(buf.highest_consecutive_key(), Some(3));
        buf.push(5).unwrap();
        assert_eq!(buf.highest_consecutive_key(), Some(3));
        buf.push(4).unwrap();
        assert_eq!(buf.highest_consecutive_key(), Some(5));
    }

    #[test]
    fn key_too_low() {
        let mut buf = SparseBuffer::new(3, |v: &usize| *v);

        buf.push(3).unwrap();
        match buf.push(2) {
            Ok(_) => panic!("didn't get error"),
            Err(InsertError::KeyTooLow(_)) => {}
            Err(_) => panic!("did get wrong error"),
        }
    }
}
