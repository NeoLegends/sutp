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
        .map_err(|e| panic!("accept error: {}", e))
        .for_each(|(conn, addr)| {
            println!("Handling conn from {}.", addr);

            let fut = conn
                .and_then(|stream| {
                    println!("accepted, reading to end...");
                    read_to_end(stream, Vec::new())
                })
                .and_then(|(stream, buf)| {
                    println!(
                        "received: {}, forwarding...",
                        String::from_utf8_lossy(&buf)
                    );
                    write_all(stream, buf)
                })
                .and_then(|(stream, _)| shutdown(stream))
                .map(move |_| println!("Connection to {} shut down.", addr))
                .or_else(|e| {
                    eprintln!("stream err: {:?}", e);
                    Ok(())
                });

            tokio::spawn(fut);
            Ok(())
        });

    tokio::run(fut);
}
