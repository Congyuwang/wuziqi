//! A wrapper to convert TCP connection into channel `Sender` and `Receiver`.
//!
//! ## feature:
//!
//! - Automatic disconnection handling: user may be guaranteed that
//!   on `send` error, `next` will eventually receive `None`.
//!
//! The wrapper may repeatedly send `Ping` message to check whether the connection
//! is still active. Any tcp write failure will result in connection close,
//! for both write and read.
//!
//! When remote connection closed the `write` side, `next()` will eventually
//! return `None`. However, the `Sender` may still be used to send messages
//! indefinitely.
//!
//! When remote connection closed the `read` side, sender will eventually return
//! error. Both sides of the connection will be closed in this situation.
//!
//! When remote connection got disconnected (network broken, power down, etc),
//! if pinging is enabled, then eventually both sides of the connection will be
//! closed due to write error.
//!
//! Dropping the `Conn` struct will close both sides of the connection.
//!
//! The following error on receiving messages will be sent to remote socket,
//! and then the connection will be closed.
//!
//! - DecodeError: fail to decode payload bytes
//! - MaxDataLengthExceeded: data payload top long
//! - DataCorrupted: checksum does not match
//! - UnknownMessageType: message type byte does not match
use crate::network::utility;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::io::BufReader;
use async_std::net::TcpStream;
use async_std::prelude::Stream;
use async_std::task;
use crc32fast::hash as checksum;
use futures::{AsyncWriteExt, StreamExt};
use std::fmt::{Debug, Formatter};
use std::io::ErrorKind;
use std::net::Shutdown;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

const NET_CHANNEL_SIZE: usize = 20;

/// Connection portal, returned by `handle_connection`.
///
/// The first type parameter is the type of messages sent,
/// the second type parameter is the type of responses received.
///
/// dropping this struct will close the connection
pub struct Conn<Msg, Rsp> {
    sender: Sender<Msg>,
    receiver: Receiver<Received<Rsp>>,
}

impl<Msg, Rsp> Conn<Msg, Rsp> {
    pub fn sender(&self) -> &Sender<Msg> {
        &self.sender
    }
}

impl<Msg, Rsp> Stream for Conn<Msg, Rsp> {
    type Item = Received<Rsp>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_next_unpin(cx)
    }
}

/// wrapper of responses received
pub enum Received<T> {
    /// normal message received
    Response(T),
    /// ping
    Ping,
    /// local socket error
    Error(ConnectionError),
    /// remote socket error: reason for connection close
    RemoteError(ConnectionError),
}

#[derive(Clone, Debug)]
pub enum ConnectionError {
    /// Attempting to send or receive over-sized data payload
    MaxDataLengthExceeded,
    /// Cannot decode message type
    UnknownMessageType,
    /// checksum incorrect
    DataCorrupted,
    /// `TryFrom<&[u8]>` returned error
    DecodeError,
    /// Cannot decode error message
    UnknownError,
}

pub fn handle_connection<Msg, Rsp>(
    tcp: TcpStream,
    ping_interval: Option<Duration>,
    max_data_size: u32,
) -> Conn<Msg, Rsp>
where
    Msg: Send + 'static + Into<Vec<u8>>,
    Rsp: Send + 'static + TryFrom<Vec<u8>>,
{
    let (msg_sender, msg_receiver) = bounded(NET_CHANNEL_SIZE);
    let (rsp_sender, rsp_receiver) = bounded(NET_CHANNEL_SIZE);
    if let Some(ping_interval) = ping_interval {
        send_ping::<Msg>(&tcp, ping_interval);
    }
    send_messages(&tcp, msg_receiver, max_data_size);
    retrieve_messages::<Msg, Rsp>(&tcp, rsp_sender, max_data_size);
    Conn {
        sender: msg_sender,
        receiver: rsp_receiver,
    }
}

enum MessageType<Msg> {
    Data(Msg),
    Ping,
    Error(ConnectionError),
}

// message types
const DATA: u8 = 0;
const PING: u8 = 100;
const ERROR: u8 = 200;

/// This function takes the ownership of the only instance of `Sender<Rsp>`.
///
/// Dropping the receiver of responses closes the *both* sides of the connection.
fn retrieve_messages<Msg, Rsp>(
    tcp: &TcpStream,
    rsp_sender: Sender<Received<Rsp>>,
    max_data_size: u32,
) where
    Msg: Send + 'static + Into<Vec<u8>>,
    Rsp: Send + 'static + TryFrom<Vec<u8>>,
{
    let mut tcp = tcp.clone();
    let inner = tcp.clone();
    task::spawn(async move {
        let mut reader = BufReader::new(inner);
        loop {
            match read_rsp::<Rsp>(&mut reader, max_data_size).await {
                Ok(Some(rsp)) => {
                    // if receiver got dropped, allow sender to send
                    if rsp_sender.send(rsp).await.is_err() {
                        let _ = tcp.shutdown(Shutdown::Read);
                        break;
                    }
                }
                // no more message to read
                Ok(None) => {
                    // allow sender to send
                    let _ = tcp.shutdown(Shutdown::Read);
                    break;
                }
                // read message got error
                Err(e) => {
                    let _ = rsp_sender.send(Received::Error(e.clone())).await;
                    let _ = write_msg::<Msg>(&mut tcp, MessageType::Error(e), 0).await;
                    let _ = tcp.shutdown(Shutdown::Both);
                    break;
                }
            }
        }
    });
}

/// This function takes the ownership of `Receiver<Msg>`.
///
/// ## Closing Connection:
/// Drop all instances of `Sender<Msg>`, and this function will close the
/// *write* side of TCP connection.
///
/// ## Error Handling:
/// On write error, this function closes *both* sides of the connection,
/// and drops `Receiver<Msg>`.
fn send_messages<Msg>(tcp: &TcpStream, mut msg_receiver: Receiver<Msg>, max_data_size: u32)
where
    Msg: Send + 'static + Into<Vec<u8>>,
{
    let mut tcp = tcp.clone();
    task::spawn(async move {
        while let Some(msg) = msg_receiver.next().await {
            let write_result =
                write_msg(&mut tcp, MessageType::Data::<Msg>(msg), max_data_size).await;
            if let Err(e) = write_result {
                match e.kind() {
                    ErrorKind::InvalidData => {}
                    ErrorKind::WriteZero => {
                        let _ = tcp.shutdown(Shutdown::Both);
                        return;
                    }
                    _ => unreachable!(),
                }
            }
        }
        let _ = tcp.shutdown(Shutdown::Both);
    });
}

/// Check if tcp connection is still alive by pinging,
/// shutdown connection pinging fail.
fn send_ping<Msg>(tcp: &TcpStream, ping_interval: Duration)
where
    Msg: Into<Vec<u8>> + Send + 'static,
{
    let mut tcp = tcp.clone();
    task::spawn(async move {
        loop {
            task::sleep(ping_interval).await;
            if write_msg(&mut tcp, MessageType::Ping::<Msg>, 0)
                .await
                .is_err()
            {
                let _ = tcp.shutdown(Shutdown::Both);
                break;
            }
        }
    });
}

/// `Ok(Some)` if read succeed.
/// `Ok(None)` if no more data to read.
///
/// `Err()` if error occurred:
/// - DecodeError: fail to decode payload bytes
/// - MaxDataLengthExceeded: data payload top long
/// - DataCorrupted: checksum does not match
/// - UnknownMessageType: message type byte does not match
///
async fn read_rsp<Rsp>(
    reader: &mut BufReader<TcpStream>,
    max_data_size: u32,
) -> std::result::Result<Option<Received<Rsp>>, ConnectionError>
where
    Rsp: TryFrom<Vec<u8>> + 'static,
{
    let packet_type = match utility::read_one_byte(reader).await {
        None => return Ok(None),
        Some(pt) => pt,
    };
    match packet_type {
        DATA => {
            let size = match utility::read_be_u32(reader).await {
                None => return Ok(None),
                Some(s) => s,
            };
            if size > max_data_size {
                Err(ConnectionError::MaxDataLengthExceeded)?
            }
            let pay_load = match utility::read_n_bytes(reader, size).await {
                None => return Ok(None),
                Some(s) => s,
            };
            let check_sum = match utility::read_be_u32(reader).await {
                None => return Ok(None),
                Some(s) => s,
            };
            if checksum(&pay_load) != check_sum {
                Err(ConnectionError::DataCorrupted)
            } else {
                match Rsp::try_from(pay_load) {
                    Ok(rsp) => Ok(Some(Received::Response(rsp))),
                    Err(_) => Err(ConnectionError::DecodeError),
                }
            }
        }
        ERROR => {
            let error_code = match utility::read_one_byte(reader).await {
                None => return Ok(None),
                Some(s) => s,
            };
            Ok(Some(Received::RemoteError(
                ConnectionError::from_error_code(error_code),
            )))
        }
        PING => Ok(Some(Received::Ping)),
        _ => Err(ConnectionError::UnknownMessageType)?,
    }
}

/// Attempt to write message to TcpStream.
///
/// On write error, return `WriteZero`.
/// If payload too large, return `InvalidData`.
async fn write_msg<Msg>(
    tcp: &mut TcpStream,
    msg: MessageType<Msg>,
    max_data_size: u32,
) -> std::io::Result<()>
where
    Msg: Into<Vec<u8>>,
{
    match msg {
        MessageType::Data(msg) => {
            let bytes = wrap_data_payload(&msg.into(), max_data_size)?;
            tcp.write_all(&bytes).await
        }
        MessageType::Error(e) => {
            let err_code = [ERROR, e.error_code()];
            tcp.write_all(&err_code).await
        }
        MessageType::Ping => tcp.write_all(&[PING]).await,
    }
}

/// Write data bytes and checksum.
///
/// structure: `[TYPE, SIZE, PAYLOAD, CHECKSUM]`
#[inline]
fn wrap_data_payload(payload: &[u8], max_data_len: u32) -> std::io::Result<Vec<u8>> {
    let size = payload.len();
    if size > max_data_len as usize {
        Err(std::io::Error::from(std::io::ErrorKind::InvalidData))?
    }
    // type + payload size + payload + checksum
    let mut dat = Vec::with_capacity(1 + 4 + size + 4);
    dat.push(DATA);
    dat.extend((size as u32).to_be_bytes());
    dat.extend(payload);
    dat.extend(checksum(payload).to_be_bytes());
    Ok(dat)
}

impl ConnectionError {
    fn error_code(&self) -> u8 {
        match self {
            // UnknownError won't get sent
            ConnectionError::UnknownError => 100,
            ConnectionError::MaxDataLengthExceeded => 200,
            ConnectionError::UnknownMessageType => 201,
            ConnectionError::DecodeError => 202,
            ConnectionError::DataCorrupted => 203,
        }
    }

    fn from_error_code(code: u8) -> Self {
        match code {
            200 => ConnectionError::MaxDataLengthExceeded,
            201 => ConnectionError::UnknownMessageType,
            202 => ConnectionError::DecodeError,
            203 => ConnectionError::DataCorrupted,
            _ => ConnectionError::UnknownError,
        }
    }
}

impl<T> Debug for Received<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Received::Response(_) => f.write_str("Responses::Response"),
            Received::Ping => f.write_str("Responses::Ping"),
            Received::Error(_) => f.write_str("Responses::Error"),
            Received::RemoteError(_) => f.write_str("Responses::RemoteError"),
        }
    }
}

#[cfg(test)]
mod test_network_module {
    use crate::lobby::token::RoomToken;
    use crate::network::connection::{handle_connection, Conn, ConnectionError, Received};
    use async_std::channel::{bounded, Receiver};
    use async_std::net::{TcpListener, TcpStream};
    use async_std::task;
    use futures::executor::block_on;
    use futures::StreamExt;
    use rand::random;
    use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4};
    use std::ops::Deref;
    use std::sync::Arc;
    use std::time::Duration;
    use std::vec;

    struct NotEmpty(Vec<u8>);

    impl TryFrom<Vec<u8>> for NotEmpty {
        type Error = ();
        fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
            if value.is_empty() {
                Err(())
            } else {
                Ok(NotEmpty(value))
            }
        }
    }

    fn test_address(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port))
    }

    fn start_server(port: u16) -> Receiver<(TcpStream, SocketAddr)> {
        let (s, conn_receiver) = bounded(1);
        task::spawn(async move {
            let server = TcpListener::bind(test_address(port)).await.unwrap();
            loop {
                let conn = server.accept().await.unwrap();
                if s.send(conn).await.is_err() {
                    break;
                }
            }
        });
        conn_receiver
    }

    fn gen_rand_bytes(number: u16, length: u8) -> Vec<Vec<u8>> {
        let mut rand_bytes = Vec::with_capacity(number as usize);
        for _ in 0..number {
            let mut buf = Vec::with_capacity(length as usize);
            for _ in 0..length {
                buf.push(random())
            }
            rand_bytes.push(buf)
        }
        rand_bytes
    }

    #[test]
    fn send_bytes_from_server_ping() {
        let mut conn = start_server(8889);
        let rand_bytes = Arc::new(gen_rand_bytes(100, 5));
        let rand_bytes_clone = rand_bytes.clone();

        // send bytes from server
        task::spawn(async move {
            let (tcp, _) = conn.next().await.unwrap();
            let server: Conn<Vec<u8>, Vec<u8>> =
                handle_connection(tcp, Some(Duration::from_millis(10)), 128);
            for bytes in rand_bytes_clone.iter() {
                task::sleep(Duration::from_millis(10)).await;
                server.sender.send(bytes.clone()).await.unwrap();
            }
        });

        // receive bytes from client
        let tcp = block_on(async move {
            task::sleep(Duration::from_millis(100)).await;
            TcpStream::connect(test_address(8889)).await
        })
        .unwrap();
        let mut client: Conn<Vec<u8>, Vec<u8>> =
            handle_connection(tcp, Some(Duration::from_millis(10)), 128);
        let responses = block_on(async move {
            let mut responses: Vec<Vec<u8>> = Vec::with_capacity(100);
            while let Some(b) = client.next().await {
                match b {
                    Received::Response(b) => responses.push(b),
                    Received::Ping => {}
                    _ => panic!("error receiving message"),
                }
            }
            responses
        });

        assert_eq!(rand_bytes.deref(), &responses)
    }

    #[test]
    fn send_bytes_from_client() {
        let mut conn = start_server(8888);
        let rand_bytes = Arc::new(gen_rand_bytes(100, 5));
        let rand_bytes_clone = rand_bytes.clone();

        // send bytes from server
        let server_future = task::spawn(async move {
            let (tcp, _) = conn.next().await.unwrap();
            let server: Conn<Vec<u8>, Vec<u8>> = handle_connection(tcp, None, 128);
            server
        });

        // receive bytes from client
        let tcp = block_on(async move {
            task::sleep(Duration::from_millis(100)).await;
            TcpStream::connect(test_address(8888)).await
        })
        .unwrap();
        let client: Conn<Vec<u8>, Vec<u8>> = handle_connection(tcp, None, 128);
        task::spawn(async move {
            for bytes in rand_bytes_clone.iter() {
                client.sender().send(bytes.clone()).await.unwrap();
            }
        });
        let responses = block_on(async move {
            let mut responses: Vec<Vec<u8>> = Vec::with_capacity(100);
            let mut server = server_future.await;
            while let Some(b) = server.next().await {
                match b {
                    Received::Response(b) => responses.push(b),
                    _ => panic!("error receiving message"),
                }
            }
            responses
        });

        assert_eq!(rand_bytes.deref(), &responses)
    }

    #[test]
    fn send_bytes_decode_fail() {
        let port: u16 = 9999;
        let mut conn = start_server(port);

        // send bytes from server
        let server_future = task::spawn(async move {
            let (tcp, _) = conn.next().await.unwrap();
            let server: Conn<Vec<u8>, Vec<u8>> = handle_connection(tcp, None, 128);
            let _ = server.sender().send(vec![0]).await;
            let _ = server.sender().send(Vec::new()).await;
            server
        });

        // receive bytes from client
        let tcp = block_on(async move {
            task::sleep(Duration::from_millis(100)).await;
            TcpStream::connect(test_address(port)).await
        })
        .unwrap();
        let mut client: Conn<Vec<u8>, NotEmpty> = handle_connection(tcp, None, 128);
        let responses = block_on(async move {
            let mut responses: Vec<Received<NotEmpty>> = Vec::with_capacity(100);
            while let Some(b) = client.next().await {
                responses.push(b);
            }
            responses
        });

        let server_msg = block_on(async move {
            let mut server = server_future.await;
            let mut msg: Vec<Received<Vec<u8>>> = Vec::with_capacity(100);
            while let Some(b) = server.next().await {
                msg.push(b);
            }
            msg
        });

        assert_eq!(responses.len(), 2);
        assert_eq!(server_msg.len(), 1);
        assert!(matches!(responses[0], Received::Response(_)));
        assert!(matches!(
            responses[1],
            Received::Error(ConnectionError::DecodeError)
        ));
        assert!(matches!(
            server_msg[0],
            Received::RemoteError(ConnectionError::DecodeError)
        ));
    }
}
