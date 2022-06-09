mod client_connection;
mod game_session;
pub(crate) mod messages;
mod room;
mod room_manager;
pub(crate) mod token;
mod user_db;

use crate::lobby::user_db::LoginValidator;
use anyhow::Result;
use async_std::net::TcpListener;
use async_std::sync::Mutex;
pub use client_connection::{ClientConnection, ConnectionInitError, ConnectionStats};
use futures_rustls::TlsAcceptor;
pub use messages::{
    CreateAccountFailure, InvalidAccountPassword, LoginFailure, Messages, Responses, RoomState,
    UpdatePasswordFailure,
};
use room_manager::RoomManager;
use rustls::ServerConfig;
use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::path::Path;
use std::sync::Arc;
pub use token::RoomToken;

pub async fn start_server(
    addrs: SocketAddrV4,
    server_config: Arc<ServerConfig>,
    db_path: &Path,
) -> Result<()> {
    let connection_stats = ConnectionStats::new();
    let user_name_set = Arc::new(Mutex::new(HashMap::new()));
    let room_manager = RoomManager::new();
    let listener = TcpListener::bind(addrs).await?;
    let acceptor = TlsAcceptor::from(server_config);
    let login_validator = LoginValidator::init(db_path)?;
    while let Ok((stream, socket)) = listener.accept().await {
        match ClientConnection::init(
            stream,
            acceptor.clone(),
            socket,
            connection_stats.clone(),
            user_name_set.clone(),
            login_validator.clone(),
        )
        .await
        {
            Ok(conn) => room_manager.accept_connection(conn),
            Err((e, Some(conn))) => {
                let _ = conn
                    .sender()
                    .send(Responses::ConnectionInitFailure(e))
                    .await;
            }
            _ => {}
        }
    }
    Ok(())
}
