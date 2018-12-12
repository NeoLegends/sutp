use futures::{Future, Stream};
use sutp::SutpListener;
use tokio::{self, io::read_to_end};

fn main() {
    let addr = "0.0.0.0:12345".parse().unwrap();
    let fut = SutpListener::bind(&addr)
        .unwrap()
        .incoming()
        .map(|(conn, _)| conn)
        .buffer_unordered(10)
        .and_then(|stream| read_to_end(stream, Vec::new()))
        .for_each(|(_, buf)| {
            println!("{}", String::from_utf8_lossy(&buf));
            Ok(())
        })
        .map_err(|e| panic!("err: {:?}", e));

    tokio::run(fut);
}
