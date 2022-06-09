use anyhow::{Error, Result};
use async_std::channel::Sender;
use async_std::io::{stdin, BufReader, Stdin};
use async_std::net::TcpStream;
use async_std::task;
use async_std::task::{block_on, JoinHandle};
use futures::{join, AsyncBufReadExt, StreamExt};
use futures_rustls;
use futures_rustls::{TlsConnector, TlsStream};
use log::{error, info, warn, LevelFilter};
use rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore};
use rustls_pemfile::certs;
use std::env;
use std::fs::File;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use webpki_roots;
use wuziqi::{Color, Conn, Messages, Received, Responses, RoomState, RoomToken, SessionConfig};

const PING_INTERVAL: Option<Duration> = Some(Duration::from_secs(5));

fn main() {
    env_logger::builder().filter_level(LevelFilter::Info).init();
    if let Err(e) = block_on(run_client()) {
        error!("client stopped on error {}", e);
    }
}

async fn run_client() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        Err(Error::msg("usage: ./client {{server domain:port}} {{root_cert}}, example: ./client congyu-test.xyz:8080"))?
    } else {
        let address = &args[1];
        // let domain = domain.to_socket_addrs().await?.next().expect("failed to resolve domain");
        let domain = address
            .splitn(2, ":")
            .next()
            .expect("failed to find domain");
        let domain = rustls::ServerName::try_from(domain).expect("failed to read domain");
        let mut root_certs = RootCertStore::empty();
        root_certs.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.into_iter().map(
            |c| {
                OwnedTrustAnchor::from_subject_spki_name_constraints(
                    c.subject,
                    c.spki,
                    c.name_constraints,
                )
            },
        ));
        if args.len() == 3 {
            let root_cert = &args[2];
            let mut reader =
                std::io::BufReader::new(File::open(root_cert).expect("failed to open root cert"));
            let cert = certs(&mut reader).expect("failed to read root cert");
            root_certs.add_parsable_certificates(&cert);
        }
        let config = Arc::new(
            ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_certs)
                .with_no_client_auth(),
        );
        let tls = TlsConnector::from(config);
        let tls = TlsStream::Client(
            tls.connect(domain, TcpStream::connect(address).await?)
                .await?,
        );
        let conn = Conn::init(tls, PING_INTERVAL, 512);
        let handle1 = accept_input(stdin(), conn.sender().clone());
        let handle2 = print_server_responses(conn);
        join!(handle1, handle2);
    }
    Ok(())
}

fn accept_input(input: Stdin, sender: Sender<Messages>) -> JoinHandle<()> {
    task::spawn(async move {
        let reader = BufReader::new(input);
        let mut lines = reader.lines();
        while let Some(line) = lines.next().await {
            match line {
                Ok(line) => {
                    if let Some(msg) = string_to_msg(&line) {
                        match msg {
                            Messages::ExitGame => {
                                let _ = sender.send(Messages::ExitGame).await;
                                break;
                            }
                            msg => {
                                if sender.send(msg).await.is_err() {
                                    info!("server closed");
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("read line error: {}", e);
                }
            }
        }
    })
}

fn print_server_responses(mut conn: Conn<Messages, Responses>) -> JoinHandle<()> {
    task::spawn(async move {
        while let Some(rsp) = conn.next().await {
            match rsp {
                Received::Response(rsp) => {
                    println!("{}", rsp_to_string(rsp));
                }
                Received::Ping => {}
                Received::Error(e) => {
                    error!("connection error: {}", e);
                    break;
                }
                Received::RemoteError(e) => {
                    error!("server side connection error: {}", e);
                    break;
                }
            }
        }
        println!("connection closed");
    })
}

fn string_to_msg(msg: &str) -> Option<Messages> {
    let msg = msg.to_lowercase();
    if msg.starts_with("new room") {
        Some(Messages::CreateRoom(SessionConfig {
            undo_request_timeout: 10,
            undo_dialogue_extra_seconds: 5,
            play_timeout: 0,
        }))
    } else if msg.starts_with("join") {
        match msg.splitn(2, " ").last() {
            None => {
                print_help();
                None
            }
            Some(token) => match RoomToken::from_code(token) {
                Ok(token) => Some(Messages::JoinRoom(token)),
                Err(e) => {
                    println!("invalid token: {}", e);
                    None
                }
            },
        }
    } else if msg.starts_with("login") {
        let cmd: Vec<String> = msg.splitn(3, " ").map(|x| x.to_string()).collect();
        if cmd.len() < 3 {
            print_help();
            None
        } else {
            Some(Messages::Login(cmd[1].clone(), cmd[2].clone()))
        }
    } else if msg.starts_with("register") {
        let cmd: Vec<String> = msg.splitn(3, " ").map(|x| x.to_string()).collect();
        if cmd.len() < 3 {
            print_help();
            None
        } else {
            Some(Messages::CreateAccount(cmd[1].clone(), cmd[2].clone()))
        }
    } else if msg.starts_with("update") {
        let cmd: Vec<String> = msg.splitn(4, " ").map(|x| x.to_string()).collect();
        if cmd.len() < 4 {
            print_help();
            None
        } else {
            Some(Messages::UpdateAccount(
                cmd[1].clone(),
                cmd[2].clone(),
                cmd[3].clone(),
            ))
        }
    } else if msg.starts_with("quit room") {
        Some(Messages::QuitRoom)
    } else if msg.starts_with("ready") {
        Some(Messages::Ready)
    } else if msg.starts_with("unready") {
        Some(Messages::Unready)
    } else if msg.starts_with("play") {
        let cmd: Vec<String> = msg.splitn(3, " ").map(|x| x.to_string()).collect();
        if cmd.len() < 3 {
            print_help();
            None
        } else {
            match (u8::from_str(&cmd[1]), u8::from_str(&cmd[2])) {
                (Ok(x), Ok(y)) => Some(Messages::Play(x, y)),
                _ => {
                    print_help();
                    None
                }
            }
        }
    } else if msg.starts_with("request undo") {
        Some(Messages::RequestUndo)
    } else if msg.starts_with("approve undo") {
        Some(Messages::ApproveUndo)
    } else if msg.starts_with("search") {
        let cmd: Vec<String> = msg.splitn(2, " ").map(|x| x.to_string()).collect();
        if cmd.len() < 2 {
            Some(Messages::SearchOnlinePlayers(None, 20))
        } else {
            Some(Messages::SearchOnlinePlayers(Some(cmd[1].clone()), 20))
        }
    } else if msg.starts_with("reject undo") {
        Some(Messages::RejectUndo)
    } else if msg.starts_with("quit session") {
        Some(Messages::QuitGameSession)
    } else if msg.starts_with("chat") {
        match msg.splitn(2, " ").last() {
            None => {
                print_help();
                None
            }
            Some(msg) => Some(Messages::ChatMessage(msg.to_string())),
        }
    } else if msg.starts_with("exit") {
        Some(Messages::ExitGame)
    } else if msg.starts_with("to") {
        let messages: Vec<&str> = msg.splitn(3, " ").collect();
        if messages.len() < 3 {
            print_help();
            None
        } else {
            Some(Messages::ToPlayer(
                messages[1].to_string(),
                Vec::from(messages[2]),
            ))
        }
    } else {
        print_help();
        None
    }
}

fn print_help() {
    println!(
        "commands:\n\
        - login name password\n\
        - register name password\n\
        - update name password\n\
        - to `player` `msg`\n\
        - new room\n\
        - search 'name'\n\
        - join 'token'\n\
        - quit room\n\
        - ready\n\
        - unready\n\
        - play 'x' 'y'\n\
        - request undo\n\
        - approve undo\n\
        - reject undo\n\
        - quit session\n\
        - chat 'msg'\n\
        - exit"
    );
}

fn rsp_to_string(rsp: Responses) -> String {
    match rsp {
        Responses::LoginSuccess(name) => format!("{} login success", name),
        Responses::ConnectionInitFailure(e) => {
            format!("connection init failure: {:?}", e)
        }
        Responses::RoomCreated(token) => {
            format!("room created! token: {}", token)
        }
        Responses::JoinRoomSuccess(token, state) => match state {
            RoomState::Empty => {
                format!("enter room {} success, the room is empty", token)
            }
            RoomState::OpponentReady(name) => {
                format!(
                    "enter room {} success. player {} is in room, and is ready",
                    token, name
                )
            }
            RoomState::OpponentUnready(name) => {
                format!(
                    "enter room {} success. player {} is in room, unready",
                    token, name
                )
            }
        },
        Responses::JoinRoomFailureTokenNotFound => "room token does not exit".to_string(),
        Responses::JoinRoomFailureRoomFull => "cannot join room. room is full.".to_string(),
        Responses::OpponentJoinRoom(name) => {
            format!("opponent ({}) joins room", name)
        }
        Responses::OpponentQuitRoom => {
            format!("opponent quits room")
        }
        Responses::OpponentReady => {
            format!("opponent is ready")
        }
        Responses::OpponentUnready => {
            format!("opponent is not ready")
        }
        Responses::GameStarted(color) => match color {
            Color::Black => format!("game started, your play X, (X first)"),
            Color::White => format!("game started, your play O, (X first)"),
        },
        Responses::FieldUpdate(f) => {
            format!("field updated:\n{:?}", f)
        }
        Responses::UndoRequest => "received undo request".to_string(),
        Responses::UndoTimeoutRejected => "undo request rejected by timeout".to_string(),
        Responses::UndoAutoRejected => "undo request invalid".to_string(),
        Responses::Undo(f) => {
            format!("undo permitted:\n{:?}", f)
        }
        Responses::UndoRejectedByOpponent => "undo request rejected".to_string(),
        Responses::GameEndBlackTimeout => "black player timeout".to_string(),
        Responses::GameEndWhiteTimeout => "white player timeout".to_string(),
        Responses::GameEndBlackWins => "black player wins".to_string(),
        Responses::GameEndWhiteWins => "white player wins".to_string(),
        Responses::GameEndDraw => "game end: Draw".to_string(),
        Responses::RoomScores((n1, p1), (n2, p2)) => {
            format!("score update ({}: {} / {}: {})", n1, p1, n2, p2)
        }
        Responses::OpponentQuitGameSession => {
            format!("opponent quit game session")
        }
        Responses::OpponentExitGame => {
            format!("opponent exit game")
        }
        Responses::OpponentDisconnected => {
            format!("opponent disconnected")
        }
        Responses::GameSessionError(e) => {
            format!("game session error {}", e)
        }
        Responses::ChatMessage(name, msg) => {
            format!("chat message from {}:\n>> {}", name, msg)
        }
        Responses::FromPlayer(name, msg) => {
            format!("from {} : {}", name, String::from_utf8(msg).unwrap())
        }
        Responses::PlayerList(names) => {
            let mut list_str = String::new();
            list_str.extend("received online players:\n".chars());
            for name in names {
                list_str.extend(format!("    - {}", name).chars());
            }
            list_str
        }
        Responses::CreateAccountFailure(e) => {
            format!("create account failure {:?}", e)
        }
        Responses::LoginFailure(e) => {
            format!("login failure {:?}", e)
        }
        Responses::UpdateAccountFailure(e) => {
            format!("update account failure {:?}", e)
        }
        Responses::CreateAccountSuccess(name, password) => {
            format!("create account success ({}, {})", name, password)
        }
        Responses::UpdateAccountSuccess(name, password) => {
            format!("update account success ({}, {})", name, password)
        }
        Responses::QuitRoomSuccess => {
            format!("quit room success")
        }
        Responses::QuitGameSessionSuccess => {
            format!("quit session success")
        }
    }
}
