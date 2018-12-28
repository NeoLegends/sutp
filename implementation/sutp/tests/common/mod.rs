use std::{fmt::Debug, sync::mpsc, time::Duration};

pub fn run_timed<
    E: 'static + Debug + Send,
    F: 'static + Send + FnOnce(mpsc::Sender<E>),
>(
    duration: Duration,
    func: F,
) {
    let (err_tx, err_rx) = mpsc::channel();

    func(err_tx);

    match err_rx.recv_timeout(duration) {
        Ok(err) => panic!("{:?}", err),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("timed out"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
    }
}
