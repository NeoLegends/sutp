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
