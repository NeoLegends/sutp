//! A server that receives data on a line-by-line basis over a given SUTP
//! port and echoes it back to the sender.

mod server;

use clap::{
    app_from_crate, crate_authors, crate_description, crate_name, crate_version,
    AppSettings, Arg,
};
use env_logger;
use log::LevelFilter;

const PORT_ARG: &str = "PORT";

fn main() {
    env_logger::Builder::new()
        .filter(Some("sutp_echo_server"), LevelFilter::Info)
        .init();

    let matches = app_from_crate!()
        .setting(AppSettings::GlobalVersion)
        .setting(AppSettings::VersionlessSubcommands)
        .arg(
            Arg::with_name(PORT_ARG)
                .short("p")
                .long("port")
                .default_value("12345")
                .takes_value(true)
                .required(true)
                .validator(|val| {
                    val.parse::<u16>().map(|_| ()).map_err(|_| {
                        format!("'{}' cannot be parsed as number.", val)
                    })
                })
                .help("The UDP Port to listen on."),
        )
        .get_matches();

    server::serve(matches.value_of(PORT_ARG).unwrap().parse().unwrap());
}
