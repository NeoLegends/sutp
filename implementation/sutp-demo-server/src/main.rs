use futures::{Future, Stream};
use sutp::SutpListener;
use tokio::{self, io::read_to_end};

fn main() {
    let addr = "0.0.0.0:12345".parse().unwrap();
    let fut = SutpListener::bind(&addr)
        .unwrap()
        .incoming()
        .map_err(|e| panic!("accept error: {}", e))
        .for_each(|(conn, addr)| {
            println!("Handling conn from {}", addr);

            let fut = conn
                .and_then(|stream| read_to_end(stream, Vec::new()))
                .and_then(|(_, buf)| {
                    println!("received: {}", String::from_utf8_lossy(&buf));
                    Ok(())
                })
                .or_else(|e| {
                    eprintln!("stream err: {:?}", e);
                    Ok(())
                });

            tokio::spawn(fut);
            Ok(())
        });

    tokio::run(fut);
}
