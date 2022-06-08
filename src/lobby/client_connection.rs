use crate::lobby::messages::{Messages, Responses};
use crate::network::connection::{Conn, ConnectionError, Received};
use async_std::channel::Sender;
use async_std::net::TcpStream;
use async_std::prelude::Stream;
use async_std::sync::Mutex;
use async_std::task::block_on;
use bincode::{Decode, Encode};
use futures::StreamExt;
use futures_rustls::{TlsAcceptor, TlsStream};
use log::{error, info};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::hash::Hash;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

const PING_INTERVAL: Duration = Duration::from_secs(5);
const MAX_DATA_SIZE: u32 = 1024 * 1024 * 20;
const SINGLE_IP_MAX_CONN: u32 = 64;
const MAX_USER_NAME_BYTES: usize = 32;
const MAX_PLAYER_SEARCH_RESULT_COUNT: usize = 20;

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum ConnectionInitError {
    TlsError,
    IpMaxConnExceed,
    ConnectionClosed,
    UserNameNotReceived,
    UserNameTooLong,
    UserNameExists,
    InvalidUserName,
    NetworkError(ConnectionError),
}

pub struct ClientConnection {
    inner: Conn<Responses, Messages>,
    player_name: String,
    player_id: u64,
    socket_address: SocketAddr,
    connection_stats: Arc<Mutex<ConnectionStats>>,
    name_dict: Arc<Mutex<HashMap<String, Sender<Responses>>>>,
}

/// Handle Client Connection
///
/// # Convention
///
/// The connection should start by sending `Messages::UserName(user_name)`,
/// otherwise the connection will return `UserNameNotReceived`.
impl ClientConnection {
    pub async fn init(
        tcp: TcpStream,
        acceptor: TlsAcceptor,
        socket_address: SocketAddr,
        connection_stats: Arc<Mutex<ConnectionStats>>,
        name_dict: Arc<Mutex<HashMap<String, Sender<Responses>>>>,
    ) -> Result<Self, (ConnectionInitError, Option<Conn<Responses, Messages>>)> {
        // add connection, check if ip max connection number exceeded
        let player_id = match connection_stats
            .lock()
            .await
            .add_conn(socket_address.clone(), SINGLE_IP_MAX_CONN)
        {
            Ok(id) => id,
            Err(e) => {
                return if let Ok(tls) = acceptor.accept(tcp).await {
                    let tls = TlsStream::Server(tls);
                    let inner = Conn::init(tls, Some(PING_INTERVAL), MAX_DATA_SIZE);
                    Err((e, Some(inner)))
                } else {
                    Err((e, None))
                };
            }
        };
        let tls = TlsStream::Server(match acceptor.accept(tcp).await {
            Ok(tls) => tls,
            Err(e) => return Err((ConnectionInitError::TlsError, None)),
        });
        let mut inner = Conn::init(tls, Some(PING_INTERVAL), MAX_DATA_SIZE);
        let player_name = loop {
            match inner.next().await {
                None => return Err((ConnectionInitError::ConnectionClosed, Some(inner))),
                Some(msg) => {
                    match msg {
                        Received::Response(msg) => match msg {
                            Messages::Login(name, password) => {
                                if name.len() > MAX_USER_NAME_BYTES {
                                    return Err((
                                        ConnectionInitError::UserNameTooLong,
                                        Some(inner),
                                    ));
                                } else {
                                    if name.is_empty() | name.contains("\n") {
                                        return Err((
                                            ConnectionInitError::InvalidUserName,
                                            Some(inner),
                                        ));
                                    } else {
                                        let mut name_dict = name_dict.lock().await;
                                        match name_dict.entry(name) {
                                            Entry::Occupied(_) => {
                                                return Err((
                                                    ConnectionInitError::UserNameExists,
                                                    Some(inner),
                                                ));
                                            }
                                            Entry::Vacant(v) => {
                                                let my_name = v.key().clone();
                                                v.insert(inner.sender().clone());
                                                break my_name;
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                return Err((ConnectionInitError::UserNameNotReceived, Some(inner)))
                            }
                        },
                        Received::Ping => {
                            // jump over Ping
                        }
                        Received::Error(e) => {
                            return Err((ConnectionInitError::NetworkError(e), Some(inner)))
                        }
                        Received::RemoteError(e) => {
                            return Err((ConnectionInitError::NetworkError(e), Some(inner)))
                        }
                    }
                }
            }
        };
        info!("player {player_id}: {player_name} connected to server from {socket_address}");
        let _ = inner
            .sender()
            .send(Responses::LoginSuccess(player_name.clone()))
            .await;
        Ok(ClientConnection {
            inner,
            player_name,
            player_id,
            socket_address,
            connection_stats,
            name_dict,
        })
    }

    pub(crate) fn sender(&self) -> &Sender<Responses> {
        self.inner.sender()
    }

    pub(crate) async fn get_online_players(&self, name: Option<String>, n: usize) -> Vec<String> {
        let name_dict = self.name_dict.lock().await;
        let n = MAX_PLAYER_SEARCH_RESULT_COUNT.min(n);
        if let Some(name) = name {
            name_dict
                .keys()
                .filter(|&x| x.contains(&name))
                .take(n)
                .map(|x| x.clone())
                .collect()
        } else {
            name_dict.keys().take(n).map(|x| x.clone()).collect()
        }
    }

    pub(crate) async fn send_to_player(&self, name: &str, msg: Vec<u8>) {
        let name_dict = self.name_dict.lock().await;
        if let Some(sender) = name_dict.get(name) {
            let _ = sender
                .send(Responses::FromPlayer(self.player_name.clone(), msg))
                .await;
        }
    }

    pub fn player_name(&self) -> &str {
        &self.player_name
    }

    pub fn player_id(&self) -> u64 {
        self.player_id
    }
}

impl Stream for ClientConnection {
    type Item = Messages;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // this loop skips `Ping`
            match self.inner.poll_next_unpin(cx) {
                Poll::Ready(msg) => {
                    match msg {
                        None => break Poll::Ready(None),
                        Some(msg) => {
                            match msg {
                                Received::Response(msg) => {
                                    if let Messages::ToPlayer(name, msg) = msg {
                                        block_on(self.send_to_player(&name, msg));
                                    } else {
                                        break Poll::Ready(Some(msg));
                                    }
                                }
                                Received::Ping => {}
                                Received::Error(e) => {
                                    // log and quit on connection error automatically
                                    let address = self.socket_address;
                                    error!("connection error ({e}) of {address}");
                                    break Poll::Ready(None);
                                }
                                Received::RemoteError(e) => {
                                    let address = self.socket_address;
                                    error!("remote connection error ({e}) of {address}");
                                    break Poll::Ready(None);
                                }
                            }
                        }
                    }
                }
                Poll::Pending => break Poll::Pending,
            }
        }
    }
}

impl Drop for ClientConnection {
    fn drop(&mut self) {
        info!(
            "player {}: {} ({}) disconnected from server",
            self.player_id, self.player_name, self.socket_address
        );
        block_on(self.connection_stats.lock()).remove_conn(self.socket_address);
        block_on(self.name_dict.lock()).remove(&self.player_name);
    }
}

/// count number of connections from each ip address
pub struct ConnectionStats {
    conn_count_v4: HashMap<Ipv4Addr, u32>,
    conn_count_v6: HashMap<Ipv6Addr, u32>,
    current_uid: u64,
}

impl ConnectionStats {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            conn_count_v4: Default::default(),
            conn_count_v6: Default::default(),
            current_uid: 0,
        }))
    }

    /// add a new connection
    fn add_conn(
        &mut self,
        socket_address: SocketAddr,
        single_ip_max_conn: u32,
    ) -> Result<u64, ConnectionInitError> {
        match socket_address {
            SocketAddr::V4(v4) => Self::add_ip(
                &mut self.current_uid,
                &mut self.conn_count_v4,
                v4.ip().clone(),
                single_ip_max_conn,
            ),
            SocketAddr::V6(v6) => Self::add_ip(
                &mut self.current_uid,
                &mut self.conn_count_v6,
                v6.ip().clone(),
                single_ip_max_conn,
            ),
        }
    }

    /// drop a connection
    fn remove_conn(&mut self, socket_address: SocketAddr) {
        match socket_address {
            SocketAddr::V4(v4) => Self::remove_ip(&mut self.conn_count_v4, v4.ip().clone()),
            SocketAddr::V6(v6) => Self::remove_ip(&mut self.conn_count_v6, v6.ip().clone()),
        }
    }

    fn add_ip<T: Eq + Hash>(
        current_uid: &mut u64,
        count_table: &mut HashMap<T, u32>,
        ip: T,
        single_ip_max_conn: u32,
    ) -> Result<u64, ConnectionInitError> {
        match count_table.entry(ip) {
            Entry::Occupied(mut o) => {
                let count = o.get_mut();
                if *count >= single_ip_max_conn {
                    Err(ConnectionInitError::IpMaxConnExceed)
                } else {
                    *count += 1;
                    *current_uid = current_uid.wrapping_add(1);
                    Ok(*current_uid)
                }
            }
            Entry::Vacant(v) => {
                v.insert(1);
                *current_uid = current_uid.wrapping_add(1);
                Ok(*current_uid)
            }
        }
    }

    fn remove_ip<T: Eq + Hash>(count_table: &mut HashMap<T, u32>, ip: T) {
        match count_table.entry(ip) {
            Entry::Occupied(mut o) => {
                let count = o.get_mut();
                if *count > 1 {
                    *count -= 1;
                } else {
                    o.remove();
                }
            }
            Entry::Vacant(_) => unreachable!(),
        }
    }
}
