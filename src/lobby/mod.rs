mod client_connection;
mod game_session;
pub(crate) mod messages;
mod room;
mod room_manager;
pub(crate) mod token;

use anyhow::Result;
use async_std::net::TcpListener;
use async_std::sync::Mutex;
pub use client_connection::{ClientConnection, ConnectionInitError, ConnectionStats};
pub use messages::{Messages, Responses, RoomState};
use room_manager::RoomManager;
use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::sync::Arc;
pub use token::RoomToken;

pub async fn start_server(addrs: SocketAddrV4) -> Result<()> {
    let connection_stats = ConnectionStats::new();
    let user_name_set = Arc::new(Mutex::new(HashSet::new()));
    let room_manager = RoomManager::new();
    let listener = TcpListener::bind(addrs).await?;
    while let Ok((stream, socket)) = listener.accept().await {
        match ClientConnection::init(
            stream,
            socket,
            connection_stats.clone(),
            user_name_set.clone(),
        )
        .await
        {
            Ok(conn) => room_manager.accept_connection(conn),
            Err((e, conn)) => {
                let _ = conn
                    .sender()
                    .send(Responses::ConnectionInitFailure(e))
                    .await;
            }
        }
    }
    Ok(())
}
