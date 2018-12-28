mod common;

use crate::common::run_timed;
use env_logger;
use futures::prelude::*;
use log::info;
use std::{
    io::{Error, ErrorKind},
    thread,
    time::Duration,
};
use sutp::{SutpListener, SutpStream};
use tokio::{
    self,
    io::{read_to_end, shutdown, write_all},
};

const TEST_STRING: &str = "Hello World";
const TEST_STRING_BIN: &[u8] = b"Hello World";

#[test]
fn hello_world_cts() {
    let _ = env_logger::try_init();

    run_timed(Duration::from_secs(5), |err_tx| {
        let addr = "127.0.0.1:12350".parse().unwrap();

        let err_tx_2 = err_tx.clone();
        let fut_srv = SutpListener::bind(&addr)
            .unwrap()
            .incoming()
            .take(1)
            .map_err(|e| panic!("accept error: {:?}", e))
            .for_each(move |(conn, _)| {
                let err_tx_3 = err_tx_2.clone();

                conn.and_then(|stream| read_to_end(stream, Vec::new()))
                    .and_then(move |(stream, buf)| {
                        let deserialized = String::from_utf8_lossy(&buf);

                        if deserialized != TEST_STRING {
                            let err_msg = format!(
                                "strings don't match: received '{}' but wanted '{}'",
                                deserialized, TEST_STRING
                            );

                            err_tx_3
                                .send(Error::new(ErrorKind::InvalidData, err_msg))
                                .unwrap();
                        }

                        shutdown(stream)
                    })
                    .map(|_| ())
            });

        let fut_client = SutpStream::connect(&addr)
            .and_then(|stream| write_all(stream, TEST_STRING_BIN))
            .and_then(|(stream, _)| shutdown(stream));

        let joined = fut_srv
            .join(fut_client)
            .map(|_| ())
            .map_err(move |e| err_tx.send(e).unwrap());

        thread::spawn(|| tokio::run(joined));
    });
}

#[test]
fn hello_world_stc() {
    let _ = env_logger::try_init();

    run_timed(Duration::from_secs(5), |err_tx| {
        let addr = "127.0.0.1:12351".parse().unwrap();

        let fut_srv = SutpListener::bind(&addr)
            .unwrap()
            .incoming()
            .take(1)
            .map_err(|e| panic!("accept error: {:?}", e))
            .for_each(|(conn, _)| {
                info!("server: accepting conn");

                conn
                    .inspect(|_| {
                        info!("server: accepted, writing {:?}", TEST_STRING_BIN)
                    })
                    .and_then(|stream| write_all(stream, TEST_STRING_BIN))
                    .inspect(|_| info!("server: written, shutting down."))
                    .and_then(|(stream, _)| shutdown(stream))
                    .map(|_| info!("server: shut down."))
            });

        let err_tx_2 = err_tx.clone();
        let fut_client = SutpStream::connect(&addr)
            .inspect(|_| info!("client: connected, reading to end."))
            .and_then(|stream| read_to_end(stream, Vec::new()))
            .inspect(|_| info!("client: parsing and shutting down"))
            .and_then(move |(stream, buf)| {
                let deserialized = String::from_utf8_lossy(&buf);

                if deserialized != TEST_STRING {
                    let err_msg = format!(
                        "strings don't match: received '{}' but wanted '{}'",
                        deserialized, TEST_STRING
                    );

                    err_tx_2
                        .send(Error::new(ErrorKind::InvalidData, err_msg))
                        .unwrap();
                }

                shutdown(stream)
            })
            .inspect(|_| info!("client: shut down."));

        let joined = fut_srv
            .join(fut_client)
            .map(|_| ())
            .map_err(move |e| err_tx.send(e).unwrap());

        thread::spawn(|| tokio::run(joined));
    });
}
