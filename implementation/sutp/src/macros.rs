macro_rules! debug_log_assert {
    ($a:expr) => {
        if !$a {
            debug!("assertion failed: {} was {}", stringify!($a), $a);
        }
    };
    ($a:expr, $msg:expr) => {
        if !$a {
            debug!($msg, stringify!($a), $a);
        }
    };
}

macro_rules! debug_log_eq {
    ($a:expr, $b:expr) => {
        if $a != $b {
            debug!("assertion failed: {} != {}", $a, $b);
        }
    };
    ($a:expr, $b:expr, $msg:expr) => {
        if $a != $b {
            debug!($msg, $a, $b);
        }
    };
}

macro_rules! ready {
    ($val:expr) => {{
        match $val {
            Async::Ready(v) => v,
            Async::NotReady => return Ok(Async::NotReady),
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
