use futures::Future;
use sutp::SutpStream;
use tokio::{
    self,
    io::{flush, read_to_end, shutdown, write_all},
};

fn main() {
    let addr = "127.0.0.1:12345".parse().unwrap();
    let fut = SutpStream::connect(&addr)
        .and_then(|stream| write_all(stream, &b"Hello World!"))
        .and_then(|(stream, _)| flush(stream))
        .and_then(|stream| {
            println!("flushed, reading to end...");
            read_to_end(stream, Vec::new())
        })
        .and_then(|(stream, data)| {
            println!("received {}", String::from_utf8_lossy(&data));
            Ok(stream)
        })
        .and_then(shutdown)
        .map_err(|e| panic!("err: {:?}", e))
        .map(|_| ());

    tokio::run(fut);
}
