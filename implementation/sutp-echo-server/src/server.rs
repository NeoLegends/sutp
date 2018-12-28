use futures::prelude::*;
use log::{info, warn};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use sutp::SutpListener;
use tokio::{
    self,
    io::{copy, shutdown},
    prelude::*,
};

pub fn serve(port: u16) {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);

    let listener = SutpListener::bind(&addr).expect("failed to bind UDP socket");
    let local_addr = listener.local_addr();

    let server_future = listener
        .incoming()
        .for_each(|(conn, addr)| {
            info!("⚡️  Accepting connection from {}.", addr);

            let client_future = conn
                .map(|stream| stream.split())
                .and_then(|(read, write)| copy(read, write))
                .and_then(|(_, _, write)| shutdown(write))
                .map(move |_| info!("🏁  Connection to {} dropped.", addr))
                .map_err(move |e| {
                    warn!("💥  Error while echoing to client {}: {:?}", addr, e)
                });

            tokio::spawn(client_future);
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| panic!("Listener error: {:?}", e));

    info!("⛓  Starting echo server on {}.", local_addr);
    tokio::run(server_future);
}
