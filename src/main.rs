pub mod game;
mod player;
mod room;
mod session;

use anyhow::{Error, Result};
use async_std::io::BufReader;
use async_std::net::{TcpListener, TcpStream};
use async_std::prelude::*;
use async_std::task;
use futures::{AsyncBufReadExt, StreamExt};
use log::warn;
use std::collections::VecDeque;
use std::mem::MaybeUninit;
use std::net::{SocketAddr, SocketAddrV4};

// TODO: to be determined
const MAX_MESSAGE_SIZE: usize = 128;
const MESSAGE_EOF: u8 = 0;

async fn accept_connection(addrs: SocketAddrV4) -> Result<()> {
    let listener = TcpListener::bind(addrs).await?;
    while let (stream, socket) = listener.accept().await? {
        task::spawn(async move {
            if let Err(e) = handle_connection(stream, socket).await {
                warn!("connection to {} unexpectedly interrupted: {}", socket, e)
            }
        });
    }
    Ok(())
}

/// might return error if connection error
async fn handle_connection(tcp: TcpStream, socket: SocketAddr) -> Result<()> {
    let mut reader = BufReader::new(tcp);
    let mut buf = Vec::with_capacity(MAX_MESSAGE_SIZE);
    loop {
        // the structure of a message: [MessageHeader; DATA; MESSAGE_EOF]
        if AsyncBufReadExt::read_until(&mut reader, MESSAGE_EOF, &mut buf).await? == 0 {
            break;
        }
        let mut buf_deque = VecDeque::from(buf);
        match buf_deque.pop_front() {
            None => Err(Error::msg(format!(
                "expect at least one byte in tcp message ({})",
                socket
            )))?,
            Some(header) => {
                // remove MESSAGE_EOF
                buf_deque.pop_back();
                task::spawn(async move {
                    // TODO: proper error handling
                    // TODO: pass tcp conn
                    distribute_message(header, buf_deque).await
                });
            }
        }
        buf = Vec::with_capacity(MAX_MESSAGE_SIZE);
    }
    // stop gracefully
    Ok(())
}

#[repr(u8)]
enum MessageHeader {
    NewRoom = 1,
    Unknown,
}

async fn distribute_message(header: u8, content: VecDeque<u8>) -> Result<()> {
    if header > 2 {
        Err(Error::msg("illegal request"))?
    }
    match header {
        1 => unimplemented!("create a new room"),
        _ => Err(Error::msg("illegal request"))?,
    }
}

fn main() {
    println!("Hello, world!");
}
