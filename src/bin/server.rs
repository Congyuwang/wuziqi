use env_logger;
use futures::executor::block_on;
use log::{error, info, LevelFilter};
use std::env;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::net::{SocketAddrV4};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use wuziqi::start_server;

fn main() {
    env_logger::builder()
        .filter_module("wuziqi", LevelFilter::Trace)
        .init();
    let args: Vec<String> = env::args().collect();
    if args.len() != 5 {
        println!("usage: ./server {{ipv4 address}} {{cert}} {{key}} {{db path}}, example: ./server 127.0.0.1:8080");
        return;
    } else {
        let ipv4 = &args[1];
        let cert = &args[2];
        let key = &args[3];
        let db_path = &args[4];
        let ipv4 =  SocketAddrV4::from_str(ipv4).expect("bad ip address");
        let mut cert = BufReader::new(File::open(cert).expect("cert not found"));
        let cert = certs(&mut cert).expect("bad cert file");
        let mut key = BufReader::new(File::open(key).expect("key not found"));
        let key_try_1 = pkcs8_private_keys(&mut key);
        key.seek(SeekFrom::Start(0)).expect("seek failure");
        let key_try_2 = rsa_private_keys(&mut key);
        let mut key = match (key_try_1, key_try_2) {
            (Err(_), Err(_)) => {
                panic!("failed to read key")
            }
            (Ok(key), _) => key,
            (_, Ok(key)) => key,
        };
        let server_config = Arc::new(ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(
                cert.into_iter().map(|c| Certificate(c)).collect(),
                PrivateKey(key.pop().expect("empty private key"))
            )
            .expect("failed to build server config"));
        info!("server started");
        if let Err(e) = block_on(start_server(ipv4, server_config, &Path::new(db_path))) {
            error!("server ended in error: {e}");
        }
    }
}
