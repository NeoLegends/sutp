/// Asserts that the given buffer has enough remaining capacity and
/// otherwise returns UnexpectedEof.
macro_rules! assert_size {
    ($buf:expr, $size:expr) => {{
        if $buf.remaining() < $size {
            return Err(::std::io::ErrorKind::UnexpectedEof.into());
        }
    }};
}

macro_rules! debug_log_assert {
    ($a:expr) => {
        if !$a {
            log::debug!("assertion failed: {} was {}", stringify!($a), $a);
        }
    };
    ($a:expr, $msg:expr) => {
        if !$a {
            log::debug!($msg, stringify!($a), $a);
        }
    };
}

macro_rules! debug_log_eq {
    ($a:expr, $b:expr) => {
        if $a != $b {
            log::debug!("assertion failed: {} != {}", $a, $b);
        }
    };
    ($a:expr, $b:expr, $msg:expr) => {
        if $a != $b {
            log::debug!($msg, $a, $b);
        }
    };
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
