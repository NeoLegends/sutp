use futures::{Future, Stream};
use log::{error, info};
use sutp::SutpListener;
use tokio::{
    self,
    io::{flush, read_exact, shutdown, write_all},
};

fn main() {
    env_logger::init();

    let addr = "0.0.0.0:12345".parse().unwrap();
    let fut = SutpListener::bind(&addr)
        .unwrap()
        .incoming()
        .map_err(|e| panic!("accept error: {}", e))
        .for_each(|(conn, addr)| {
            info!("Handling conn from {}.", addr);

            let fut = conn
                .and_then(|stream| {
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
                });

            tokio::spawn(fut);
            Ok(())
        });

    tokio::run(fut);
}
