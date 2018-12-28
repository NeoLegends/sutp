use futures::Future;
use log::info;
use sutp::SutpStream;
use tokio::{
    self,
    io::{flush, read_to_end, shutdown, write_all},
};

fn main() {
    env_logger::init();

    let addr = "127.0.0.1:12345".parse().unwrap();
    let fut = SutpStream::connect(&addr)
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

    tokio::run(fut);
}
