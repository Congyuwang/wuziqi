use env_logger;
use futures::executor::block_on;
use log::{error, info, LevelFilter};
use std::env;
use std::net::SocketAddrV4;
use std::str::FromStr;
use wuziqi::start_server;

fn main() {
    env_logger::builder()
        .filter_module("wuziqi", LevelFilter::Trace)
        .init();
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("usage: ./server {{ipv4 address}}, example: ./server 127.0.0.1:8080");
        return;
    } else {
        match SocketAddrV4::from_str(args.last().unwrap()) {
            Ok(ipv4) => {
                info!("server started");
                if let Err(e) = block_on(start_server(ipv4)) {
                    error!("server ended in error: {e}");
                }
            }
            Err(e) => {
                println!("bad ipv4 address: {}", e);
            }
        }
    }
}
