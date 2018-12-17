use env_logger;
use futures::{Future, Stream};
use sutp::SutpListener;
use tokio::{
    self,
    io::{read_to_end, shutdown, write_all},
};

fn main() {
    env_logger::init();

    let addr = "0.0.0.0:12345".parse().unwrap();
    let fut = SutpListener::bind(&addr)
        .unwrap()
        .incoming()
        .map(|(conn, addr)| {
            println!("Accepting a connection from {}", addr);

            conn.and_then(|stream| read_to_end(stream, Vec::new()))
                .inspect(move |(_, buf)| println!(
                    "Got '{}' from {}. Echoing back...",
                    String::from_utf8_lossy(&buf),
                    addr,
                ))
                .and_then(|(stream, buf)| write_all(stream, buf))
                .and_then(|(stream, _)| shutdown(stream))
        })
        .buffer_unordered(10)
        .for_each(|_| Ok(()))
        .map_err(|e| panic!("err: {:?}", e));

    tokio::run(fut);
}
