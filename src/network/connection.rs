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
use async_std::task::JoinHandle;
use bincode::{Decode, Encode};
use crc32fast::hash as checksum;
use futures::channel::oneshot;
use futures::io::{ReadHalf, WriteHalf};
use futures::{select, AsyncWriteExt, StreamExt};
use futures::{AsyncReadExt, FutureExt};
use tokio_rustls::TlsStream;
use std::fmt::{Debug, Display, Formatter};
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
/// dropping this struct and all senders will close the connection
pub struct Conn<Msg, Rsp> {
    sender: Sender<Msg>,
    receiver: Receiver<Received<Rsp>>,
}

impl<Msg, Rsp> Conn<Msg, Rsp>
where
    Msg: Send + 'static + Into<Vec<u8>>,
    Rsp: Send + 'static + TryFrom<Vec<u8>>,
{
    pub fn init(
        tls: TlsStream<TcpStream>,
        ping_interval: Option<Duration>,
        max_data_size: u32,
    ) -> Self {
        handle_connection(tls, ping_interval, max_data_size)
    }

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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
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

impl Display for ConnectionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::MaxDataLengthExceeded => f.write_str("max data length exceeded"),
            ConnectionError::UnknownMessageType => f.write_str("unknown message type"),
            ConnectionError::DataCorrupted => f.write_str("data corrupted"),
            ConnectionError::DecodeError => f.write_str("decode error"),
            ConnectionError::UnknownError => f.write_str("unknown error"),
        }
    }
}

fn handle_connection<Msg, Rsp>(
    tls: TlsStream<TcpStream>,
    ping_interval: Option<Duration>,
    max_data_size: u32,
) -> Conn<Msg, Rsp>
where
    Msg: Send + 'static + Into<Vec<u8>>,
    Rsp: Send + 'static + TryFrom<Vec<u8>>,
{
    let (msg_sender, mut inner_msg_receiver) = bounded(NET_CHANNEL_SIZE);
    let (inner_msg_sender, msg_receiver) = bounded(NET_CHANNEL_SIZE);
    let (rsp_sender, rsp_receiver) = bounded(NET_CHANNEL_SIZE);
    let inner_ping_sender = inner_msg_sender.clone();
    let (read_tls, write_tls) = tls.split();
    // define three stoppers for stopping three tasks
    let (ping_stopper, stop_pinging) = oneshot::channel::<()>();
    let (send_stopper, stop_sending) = oneshot::channel::<()>();
    let (recv_stopper, stop_receiving) = oneshot::channel::<()>();
    task::spawn(async move {
        // wrap messages from `msg_sender` with `MessageType::Data`
        while let Some(msg) = inner_msg_receiver.next().await {
            if inner_msg_sender.send(MessageType::Data(msg)).await.is_err() {
                break;
            }
        }
        ping_stopper.send(())
    });
    // start pinging task
    if let Some(ping_interval) = ping_interval {
        send_ping::<Msg>(inner_ping_sender, stop_pinging, ping_interval);
    }
    // start messages sender loop
    let send_joiner = send_messages::<Msg>(write_tls, msg_receiver, stop_sending, max_data_size);
    // start messages receiver loop
    let receive_joiner =
        retrieve_messages::<Msg, Rsp>(read_tls, rsp_sender, stop_receiving, max_data_size);
    // deal with connection shutdown
    task::spawn(async move {
        let mut recv_stopper = Some(recv_stopper);
        let mut send_stopper = Some(send_stopper);
        let mut send_joiner = send_joiner.fuse();
        let mut receive_joiner = receive_joiner.fuse();
        let (mut write_half, mut wr_err) = (None, None);
        let (mut read_half, mut r_err) = (None, None);
        // gracefully shutdown tls connections (send error messages and so on)
        loop {
            select! {
                send_result = send_joiner => {
                    let (mut wr, shut, err) = send_result;
                    match shut {
                        Some(Shutdown::Both) => {
                            if let Some(stp) = recv_stopper.take() {
                                let _ = stp.send(());
                            }
                        }
                        Some(Shutdown::Write) => {
                            let _ = wr.close().await;
                        }
                        _ => {}
                    }
                    // do not need to do anything specific
                    (write_half, wr_err) = (Some(wr), err);
                }
                receive_result = receive_joiner => {
                    let (rr, shut, err) = receive_result;
                    if let Some(Shutdown::Both) = shut {
                        if let Some(stp) = send_stopper.take() {
                            let _ = stp.send(());
                        }
                    }
                    (read_half, r_err) = (Some(rr), err);
                }
            }
            if let (Some(_), Some(_)) = (&write_half, &read_half) {
                break;
            }
        }
        let (mut write_half, _) = (write_half.unwrap(), read_half.unwrap());
        if let Some(e) = wr_err {
            let _ = write_msg(&mut write_half, MessageType::Error::<Msg>(e), u32::MAX).await;
        }
        if let Some(e) = r_err {
            let _ = write_msg(&mut write_half, MessageType::Error::<Msg>(e), u32::MAX).await;
        }
        let _ = write_half.close().await;
    });
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

/// This task returns in three possible ways:
/// - the receiver of the retrieved message is dropped: shutdown read
/// - remote write closed (eof read): shutdown read
/// - data decode error: shutdown both sides
fn retrieve_messages<Msg, Rsp>(
    read_tls: ReadHalf<TlsStream<TcpStream>>,
    rsp_sender: Sender<Received<Rsp>>,
    stop_receiving: oneshot::Receiver<()>,
    max_data_size: u32,
) -> JoinHandle<(
    ReadHalf<TlsStream<TcpStream>>,
    Option<Shutdown>,
    Option<ConnectionError>,
)>
where
    Msg: Send + 'static + Into<Vec<u8>>,
    Rsp: Send + 'static + TryFrom<Vec<u8>>,
{
    task::spawn(async move {
        let mut reader = BufReader::new(read_tls);
        let mut stop_receiving = stop_receiving.fuse();
        let (shut, err) = loop {
            select! {
                _ = stop_receiving => {
                    break (None, None);
                }
                read = read_rsp::<Rsp>(&mut reader, max_data_size).fuse() => {
                    match read {
                        Ok(Some(rsp)) => {
                            // if receiver got dropped, allow sender to send
                            if rsp_sender.send(rsp).await.is_err() {
                                break (Some(Shutdown::Read), None);
                            }
                        }
                        // no more message to read (might be due to io error or connection close)
                        Ok(None) => {
                            // allow sender to send
                            break (Some(Shutdown::Read), None);
                        }
                        // read message got error
                        Err(e) => {
                            let _ = rsp_sender.send(Received::Error(e.clone())).await;
                            break (Some(Shutdown::Both), Some(e));
                        }
                    }
                }
            }
        };
        (reader.into_inner(), shut, err)
    })
}

/// This function takes the ownership of `Receiver<Msg>`.
///
/// send_messages finishes in three different situations:
/// - all `Senders` of messages being dropped: shutdown write
/// - send data larger than limit: shutdown both
/// - remote disconnection (write failure): shutdown both
fn send_messages<Msg>(
    mut write_tls: WriteHalf<TlsStream<TcpStream>>,
    msg_receiver: Receiver<MessageType<Msg>>,
    stop_sending: oneshot::Receiver<()>,
    max_data_size: u32,
) -> JoinHandle<(
    WriteHalf<TlsStream<TcpStream>>,
    Option<Shutdown>,
    Option<ConnectionError>,
)>
where
    Msg: Send + 'static + Into<Vec<u8>>,
{
    task::spawn(async move {
        let mut stop_sending = stop_sending.fuse();
        let mut msg_receiver = msg_receiver.fuse();
        loop {
            select! {
                _ = stop_sending => {
                    break (write_tls, None, None)
                }
                msg = msg_receiver.next() => {
                    if let Some(msg) = msg {
                        let write_result = write_msg(&mut write_tls, msg, max_data_size).await;
                        if let Err(e) = write_result {
                            break match e.kind() {
                                ErrorKind::InvalidData => (
                                    write_tls,
                                    Some(Shutdown::Both),
                                    Some(ConnectionError::MaxDataLengthExceeded),
                                ),
                                // pinging disconnection
                                _ => (write_tls, Some(Shutdown::Both), None),
                            };
                        }
                    } else {
                        break (write_tls, Some(Shutdown::Write), None)
                    }
                }
            }
        }
    })
}

fn send_ping<Msg>(
    ping_sender: Sender<MessageType<Msg>>,
    stop_pinging: oneshot::Receiver<()>,
    ping_interval: Duration,
) where
    Msg: Into<Vec<u8>> + Send + 'static,
{
    task::spawn(async move {
        let mut stop_pinging = stop_pinging.fuse();
        let mut ping_sleeper = Box::pin(task::sleep(ping_interval).fuse());
        loop {
            select! {
                _ = stop_pinging => break Ok(()),
                _ = ping_sleeper => {
                    if ping_sender.send(MessageType::Ping).await.is_err() {
                        break Err(());
                    } else {
                        ping_sleeper = Box::pin(task::sleep(ping_interval).fuse());
                    }
                }
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
    reader: &mut BufReader<ReadHalf<TlsStream<TcpStream>>>,
    max_data_size: u32,
) -> Result<Option<Received<Rsp>>, ConnectionError>
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
                return Err(ConnectionError::MaxDataLengthExceeded);
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
    tls: &mut WriteHalf<TlsStream<TcpStream>>,
    msg: MessageType<Msg>,
    max_data_size: u32,
) -> std::io::Result<()>
where
    Msg: Into<Vec<u8>>,
{
    match msg {
        MessageType::Data(msg) => {
            let bytes = wrap_data_payload(&msg.into(), max_data_size)?;
            tls.write_all(&bytes).await?;
            tls.flush().await
        }
        MessageType::Error(e) => {
            let err_code = [ERROR, e.error_code()];
            tls.write_all(&err_code).await?;
            tls.flush().await
        }
        MessageType::Ping => {
            tls.write_all(&[PING]).await?;
            tls.flush().await
        }
    }
}

/// Write data bytes and checksum.
///
/// structure: `[TYPE, SIZE, PAYLOAD, CHECKSUM]`
#[inline]
fn wrap_data_payload(payload: &[u8], max_data_len: u32) -> std::io::Result<Vec<u8>> {
    let size = payload.len();
    if size > max_data_len as usize {
        Err(std::io::Error::from(ErrorKind::InvalidData))?
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

impl<T> Debug for Received<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Received::Response(rsp) => f.write_str(&format!("Responses::Response({:?})", rsp)),
            Received::Ping => f.write_str("Responses::Ping"),
            Received::Error(_) => f.write_str("Responses::Error"),
            Received::RemoteError(_) => f.write_str("Responses::RemoteError"),
        }
    }
}

#[cfg(test)]
mod test_network_module {
    use crate::network::connection::{handle_connection, Conn, ConnectionError, Received};
    use async_std::channel::{bounded, Receiver};
    use async_std::net::{TcpListener, TcpStream};
    use async_std::task;
    use futures::executor::block_on;
    use futures::StreamExt;
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::{TlsConnector, TlsStream};
    use lazy_static::lazy_static;
    use rand::random;
    use rustls::{Certificate, ClientConfig, PrivateKey, RootCertStore, ServerConfig, ServerName};
    use rustls_pemfile::{certs, pkcs8_private_keys};
    use std::fs::File;
    use std::io::BufReader;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::ops::Deref;
    use std::path::PathBuf;
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

    fn test_domain() -> &'static str {
        "localhost"
    }

    fn test_cert_folder() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test-certs");
        d
    }

    fn server_config() -> Arc<ServerConfig> {
        let mut cert_path = test_cert_folder();
        let mut key_path = test_cert_folder();
        cert_path.push("end.cert");
        key_path.push("end.rsa");
        let mut cert_reader = BufReader::new(File::open(cert_path).unwrap());
        let mut key_reader = BufReader::new(File::open(key_path).unwrap());
        let cert: Vec<Certificate> = certs(&mut cert_reader)
            .unwrap()
            .into_iter()
            .map(|v| Certificate(v))
            .collect();
        let key = PrivateKey(pkcs8_private_keys(&mut key_reader).unwrap().pop().unwrap());
        let config = ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .unwrap();
        Arc::new(config)
    }

    fn client_config() -> Arc<ClientConfig> {
        let mut chain_path = test_cert_folder();
        chain_path.push("end.chain");
        let mut chain_reader = BufReader::new(File::open(chain_path).unwrap());
        let mut root_certs = RootCertStore::empty();
        root_certs.add_parsable_certificates(&certs(&mut chain_reader).unwrap());
        let config = ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_certs)
            .with_no_client_auth();
        Arc::new(config)
    }

    lazy_static! {
        static ref SERVER_CONFIG: Arc<ServerConfig> = server_config();
        static ref CLIENT_CONFIG: Arc<ClientConfig> = client_config();
    }

    fn start_server(port: u16) -> Receiver<(TlsStream<TcpStream>, SocketAddr)> {
        let (s, conn_receiver) = bounded(1);
        let acceptor = TlsAcceptor::from(SERVER_CONFIG.clone());
        task::spawn(async move {
            let server = TcpListener::bind(test_address(port)).await.unwrap();
            loop {
                let (tcp, sock) = server.accept().await.unwrap();
                let tls = acceptor.accept(tcp).await.unwrap();
                if s.send((TlsStream::Server(tls), sock)).await.is_err() {
                    break;
                }
            }
        });
        conn_receiver
    }

    async fn client_tls(tcp: TcpStream) -> TlsStream<TcpStream> {
        let connector = TlsConnector::from(CLIENT_CONFIG.clone());
        let tls = connector
            .connect(ServerName::try_from(test_domain()).unwrap(), tcp)
            .await
            .unwrap();
        TlsStream::Client(tls)
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
        let server_life = task::spawn(async move {
            let (tls, _) = conn.next().await.unwrap();
            let server: Conn<Vec<u8>, Vec<u8>> =
                handle_connection(tls, Some(Duration::from_millis(10)), 128);
            for bytes in rand_bytes_clone.iter() {
                task::sleep(Duration::from_millis(10)).await;
                server.sender().send(bytes.clone()).await.unwrap();
            }
            server
        });

        // receive bytes from client
        let tls = block_on(async move {
            task::sleep(Duration::from_millis(100)).await;
            let tcp = TcpStream::connect(test_address(8889)).await.unwrap();
            client_tls(tcp).await
        });
        let mut client: Conn<Vec<u8>, Vec<u8>> =
            handle_connection(tls, Some(Duration::from_millis(10)), 128);
        let responses = block_on(async move {
            let mut responses: Vec<Vec<u8>> = Vec::with_capacity(100);
            while let Some(b) = client.next().await {
                match b {
                    Received::Response(b) => responses.push(b),
                    Received::Ping => {}
                    _ => panic!("error receiving message"),
                }
                if responses.len() >= 100 {
                    break;
                }
            }
            responses
        });

        drop(server_life);

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
        let tls = block_on(async move {
            task::sleep(Duration::from_millis(100)).await;
            let tcp = TcpStream::connect(test_address(8888)).await.unwrap();
            client_tls(tcp).await
        });
        let client: Conn<Vec<u8>, Vec<u8>> = handle_connection(tls, None, 128);
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

        let tls = block_on(async move {
            task::sleep(Duration::from_millis(100)).await;
            let tcp = TcpStream::connect(test_address(port)).await.unwrap();
            client_tls(tcp).await
        });

        let mut client: Conn<Vec<u8>, NotEmpty> = handle_connection(tls, None, 128);
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
