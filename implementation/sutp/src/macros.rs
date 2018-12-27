/// Asserts that the given buffer has enough remaining capacity and
/// otherwise returns UnexpectedEof.
macro_rules! assert_size {
    ($buf:expr, $size:expr) => {{
        if $buf.remaining() < $size {
            return Err(::std::io::ErrorKind::UnexpectedEof.into());
        }
    }};
}

/// Converts from Async::NotReady to ErrorKind::WouldBlock.
macro_rules! try_would_block {
    ($val:expr) => {{
        match $val? {
            Async::Ready(result) => Ok(result),
            Async::NotReady => Err(ErrorKind::WouldBlock.into()),
        }
    }};
}
