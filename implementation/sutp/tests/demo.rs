mod common;

use crate::common::run_timed;
use futures::prelude::*;
use log::{error, info};
use std::{thread, time::Duration};
use sutp::{SutpListener, SutpStream};
use tokio::{
    self,
    io::{flush, read_exact, read_to_end, shutdown, write_all},
};

#[test]
fn demo() {
    env_logger::init();

    run_timed(Duration::from_secs(5), |err_tx| {
        let addr = "127.0.0.1:12362".parse().unwrap();

        let fut_client = SutpStream::connect(&addr)
            .and_then(|stream| write_all(stream, b"Hello"))
            .inspect(|_| info!("written request"))
            .and_then(|(stream, _)| flush(stream))
            .inspect(|_| info!("flushed"))
            .and_then(|stream| read_to_end(stream, Vec::new()))
            .and_then(|(stream, buf)| {
                info!("Received back '{}'.", String::from_utf8(buf).unwrap());
                shutdown(stream)
            })
            .map_err(|e| panic!("err: {:?}", e))
            .map(|_| info!("done"));

        let fut_srv = SutpListener::bind(&addr)
            .unwrap()
            .incoming()
            .take(1)
            .map_err(|e| panic!("accept error: {}", e))
            .for_each(|(conn, addr)| {
                info!("Handling conn from {}.", addr);

                conn.and_then(|stream| {
                    info!("accepted, reading to end...");
                    read_exact(stream, vec![0; 5])
                })
                .and_then(|(stream, buf)| {
                    info!(
                        "received: {}, forwarding...",
                        String::from_utf8_lossy(&buf)
                    );
                    write_all(stream, buf)
                })
                .and_then(|(stream, _)| flush(stream))
                .and_then(shutdown)
                .map(move |_| info!("Connection to {} shut down.", addr))
                .or_else(|e| {
                    error!("stream err: {:?}", e);
                    Ok(())
                })
            });

        let joined = fut_srv
            .join(fut_client)
            .map(|_| ())
            .map_err(move |e| err_tx.send(e).unwrap());

        thread::spawn(|| tokio::run(joined));
    });
}
