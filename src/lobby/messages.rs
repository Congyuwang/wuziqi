//! Implementation principles.
//! - disconnection without clear exit signal is considered as disconnection.
use crate::game::Color::{Black, White};
use crate::game::{
    compress_field, decompress_field, Color, FieldState, FieldStateNullable, SessionConfig,
};
use crate::lobby::client_connection::ConnectionInitError;
use crate::lobby::token::RoomToken;
use crate::network::connection::ConnectionError;
use anyhow::{Error, Result};
use unroll::unroll_for_loops;
use serde::{Serialize, Deserialize};

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Messages {
    ToPlayer(String, Vec<u8>),
    /// send user name
    UserName(String),
    /// create a new room
    CreateRoom(SessionConfig),
    /// attempt to join a room with a RoomToken
    JoinRoom(RoomToken),
    /// Quit a room
    QuitRoom,
    /// when in a Room, get ready for a game session
    Ready,
    /// reverse `ready`
    Unready,
    /// play a position in game [0, 15). Out of bounds are ignored.
    /// Repeatedly playing on an occupied position will result in `GameError`.
    Play(u8, u8),
    /// request undo in game.
    RequestUndo,
    /// approve undo requests in game.
    ApproveUndo,
    /// reject undo requests in game.
    RejectUndo,
    /// quit game session (only quit this round).
    QuitGameSession,
    /// chat message
    ChatMessage(String),
    /// exit game (quit game and room), close connection
    /// exiting game without sending `ExitGame` signal is considered `Disconnected`
    ExitGame,
    /// client error: other errors excluding network error
    ClientError(String),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum RoomState {
    Empty,
    OpponentReady(String),
    OpponentUnready(String),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Responses {
    FromPlayer(String, Vec<u8>),
    /// Connection success
    ConnectionSuccess,
    /// Connection Init Error
    ConnectionInitFailure(ConnectionInitError),
    /// response to `CreateRoom`
    RoomCreated(String),
    /// response to `JoinRoom`
    /// the two fields are correspondingly
    /// `room` token
    JoinRoomSuccess(String, RoomState),
    /// response to `JoinRoom`
    JoinRoomFailureTokenNotFound,
    /// response to `JoinRoom`
    JoinRoomFailureRoomFull,
    /// when the other player gets `JoinRoomSuccess`
    /// the `String` is the username
    OpponentJoinRoom(String),
    /// when the other player `QuitRoom`
    OpponentQuitRoom,
    /// when the other player is `Ready`
    OpponentReady,
    /// when the other play does `Unready`
    OpponentUnready,
    /// when both players are `Ready`
    GameStarted(Color),
    /// update field
    FieldUpdate(FieldState),
    /// opponent request undo
    UndoRequest,
    /// undo rejected by timeout
    UndoTimeoutRejected,
    /// undo rejected due to synchronization reason
    UndoAutoRejected,
    /// undo approved
    Undo(FieldStateNullable),
    /// undo rejected by opponent
    UndoRejectedByOpponent,
    /// game session ends, black timeout
    GameEndBlackTimeout,
    /// game session ends, white timeout
    GameEndWhiteTimeout,
    /// game session ends, black wins
    GameEndBlackWins,
    /// game session ends, white wins
    GameEndWhiteWins,
    /// game session ends, draw
    GameEndDraw,
    /// Room score information (player1, player2)
    RoomScores((String, u16), (String, u16)),
    /// opponent quit game session
    OpponentQuitGameSession,
    /// opponent exit game
    OpponentExitGame,
    /// opponent disconnected
    OpponentDisconnected,
    /// game session ends in error
    GameSessionError(String),
    /// ChatMessage: (user_name, message)
    ChatMessage(String, String),
}

impl Messages {
    fn message_type(&self) -> u8 {
        match &self {
            Messages::CreateRoom(_) => 0,
            Messages::JoinRoom(_) => 1,
            Messages::QuitRoom => 2,
            Messages::Ready => 3,
            Messages::Unready => 4,
            Messages::Play(_, _) => 5,
            Messages::RequestUndo => 6,
            Messages::ApproveUndo => 7,
            Messages::RejectUndo => 8,
            Messages::QuitGameSession => 9,
            Messages::ExitGame => 10,
            Messages::ClientError(_) => 12,
            Messages::UserName(_) => 100,
            Messages::ToPlayer(_, _) => 110,
            Messages::ChatMessage(_) => 200,
        }
    }
}

impl Into<Vec<u8>> for Messages {
    fn into(self) -> Vec<u8> {
        match self {
            Messages::QuitRoom
            | Messages::Ready
            | Messages::Unready
            | Messages::RequestUndo
            | Messages::ApproveUndo
            | Messages::RejectUndo
            | Messages::QuitGameSession
            | Messages::ExitGame => [self.message_type()].to_vec(),
            Messages::CreateRoom(ref conf) => {
                let mut dat = Vec::with_capacity(25);
                dat.push(self.message_type());
                dat.extend(conf.undo_request_timeout.to_be_bytes());
                dat.extend(conf.undo_dialogue_extra_seconds.to_be_bytes());
                dat.extend(conf.play_timeout.to_be_bytes());
                dat
            }
            Messages::ToPlayer(ref name, ref msg) => {
                let mut dat = Vec::with_capacity(3 + name.len() + msg.len());
                dat.push(self.message_type());
                dat.extend((name.len() as u16).to_be_bytes());
                dat.extend(name.as_bytes());
                dat.extend(msg);
                dat
            }
            Messages::JoinRoom(ref token) => {
                let token_string = token.as_code();
                let mut dat = Vec::new();
                dat.push(self.message_type());
                dat.extend(token_string.as_bytes());
                dat
            }
            Messages::Play(x, y) => [self.message_type(), x, y].to_vec(),
            Messages::UserName(ref name) => {
                let mut dat = Vec::new();
                dat.push(self.message_type());
                dat.extend(name.as_bytes());
                dat
            }
            Messages::ChatMessage(ref msg) => {
                let mut dat = Vec::new();
                dat.push(self.message_type());
                dat.extend(msg.as_bytes());
                dat
            }
            Messages::ClientError(ref msg) => {
                let mut dat = Vec::new();
                dat.push(self.message_type());
                dat.extend(msg.as_bytes());
                dat
            }
        }
    }
}
impl TryFrom<Vec<u8>> for Messages {
    type Error = anyhow::Error;

    fn try_from(bytes: Vec<u8>) -> Result<Self> {
        match bytes[0] {
            0 => {
                if bytes.len() != 25 {
                    Err(Error::msg(
                        "client message decode error, incorrect byte length",
                    ))?
                }
                Ok(Messages::CreateRoom(SessionConfig {
                    undo_request_timeout: u64::from_be_bytes((&bytes[1..9]).try_into().unwrap()),
                    undo_dialogue_extra_seconds: u64::from_be_bytes(
                        (&bytes[9..17]).try_into().unwrap(),
                    ),
                    play_timeout: u64::from_be_bytes((&bytes[17..25]).try_into().unwrap()),
                }))
            }
            1 => {
                let token = decode_utf8_string(&bytes)?;
                Ok(Messages::JoinRoom(RoomToken::from_code(&token)?))
            }
            2 => Ok(Messages::QuitRoom),
            3 => Ok(Messages::Ready),
            4 => Ok(Messages::Unready),
            5 => {
                if bytes.len() != 3 {
                    Err(Error::msg(
                        "client message decode error, incorrect byte length",
                    ))?
                }
                Ok(Messages::Play(bytes[1], bytes[2]))
            }
            6 => Ok(Messages::RequestUndo),
            7 => Ok(Messages::ApproveUndo),
            8 => Ok(Messages::RejectUndo),
            9 => Ok(Messages::QuitGameSession),
            10 => Ok(Messages::ExitGame),
            12 => Ok(Messages::ClientError(decode_utf8_string(&bytes)?)),
            100 => Ok(Messages::UserName(decode_utf8_string(&bytes)?)),
            110 => {
                if bytes.len() < 3 {
                    Err(Error::msg(
                        "client message decode error, incorrect byte length",
                    ))?
                }
                let name_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
                if bytes.len() < 3 + name_len {
                    Err(Error::msg(
                        "client message decode error, incorrect byte length",
                    ))?
                }
                let name = String::from_utf8(bytes[3..(3 + name_len)].to_vec())
                    .map_err(|_| Error::msg("utf-8 decode error"))?;
                let msg = bytes[(3 + name_len)..].to_vec();
                Ok(Messages::ToPlayer(name, msg))
            }
            200 => Ok(Messages::ChatMessage(decode_utf8_string(&bytes)?)),
            _ => Err(Error::msg("client messages decode error")),
        }
    }
}

impl Responses {
    fn response_type(&self) -> u8 {
        match &self {
            Responses::RoomCreated(_) => 0,
            Responses::JoinRoomSuccess(_, _) => 1,
            Responses::JoinRoomFailureTokenNotFound => 2,
            Responses::JoinRoomFailureRoomFull => 3,
            Responses::OpponentJoinRoom(_) => 4,
            Responses::OpponentQuitRoom => 5,
            Responses::OpponentReady => 6,
            Responses::OpponentUnready => 7,
            Responses::GameStarted(_) => 8,
            Responses::FieldUpdate(_) => 9,
            Responses::UndoRequest => 10,
            Responses::UndoTimeoutRejected => 11,
            Responses::UndoAutoRejected => 12,
            Responses::Undo(_) => 13,
            Responses::UndoRejectedByOpponent => 14,
            Responses::GameEndBlackTimeout => 15,
            Responses::GameEndWhiteTimeout => 16,
            Responses::GameEndBlackWins => 17,
            Responses::GameEndWhiteWins => 18,
            Responses::GameEndDraw => 19,
            Responses::OpponentQuitGameSession => 20,
            Responses::OpponentExitGame => 21,
            Responses::OpponentDisconnected => 22,
            Responses::GameSessionError(_) => 23,
            Responses::RoomScores(_, _) => 24,
            Responses::ConnectionSuccess => 100,
            Responses::ConnectionInitFailure(_) => 110,
            Responses::FromPlayer(_, _) => 120,
            Responses::ChatMessage(_, _) => 200,
        }
    }
}

impl Into<Vec<u8>> for Responses {
    fn into(self) -> Vec<u8> {
        match &self {
            Responses::JoinRoomFailureTokenNotFound
            | Responses::JoinRoomFailureRoomFull
            | Responses::OpponentQuitRoom
            | Responses::OpponentReady
            | Responses::OpponentUnready
            | Responses::UndoRequest
            | Responses::UndoTimeoutRejected
            | Responses::UndoAutoRejected
            | Responses::UndoRejectedByOpponent
            | Responses::GameEndBlackTimeout
            | Responses::GameEndWhiteTimeout
            | Responses::GameEndBlackWins
            | Responses::GameEndWhiteWins
            | Responses::GameEndDraw
            | Responses::OpponentQuitGameSession
            | Responses::OpponentExitGame
            | Responses::ConnectionSuccess
            | Responses::OpponentDisconnected => [self.response_type()].to_vec(),
            Responses::ConnectionInitFailure(e) => {
                let mut dat = Vec::with_capacity(2);
                dat.push(self.response_type());
                dat.push(match e {
                    ConnectionInitError::IpMaxConnExceed => 1,
                    ConnectionInitError::ConnectionClosed => 2,
                    ConnectionInitError::UserNameNotReceived => 3,
                    ConnectionInitError::UserNameTooLong => 4,
                    ConnectionInitError::UserNameExists => 5,
                    ConnectionInitError::InvalidUserName => 6,
                    ConnectionInitError::NetworkError(_) => 7,
                });
                dat
            }
            Responses::JoinRoomSuccess(token, room_state) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                let token_bytes = token.as_bytes();
                dat.extend((token_bytes.len() as u16).to_be_bytes());
                dat.extend(token.as_bytes());
                match room_state {
                    RoomState::Empty => dat.push(0),
                    RoomState::OpponentUnready(name) => {
                        dat.push(1);
                        dat.extend(name.as_bytes());
                    }
                    RoomState::OpponentReady(name) => {
                        dat.push(2);
                        dat.extend(name.as_bytes());
                    }
                }
                dat
            }
            Responses::RoomCreated(token) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                dat.extend(token.as_bytes());
                dat
            }
            Responses::GameStarted(c) => match c {
                Color::Black => [self.response_type(), 0].to_vec(),
                Color::White => [self.response_type(), 1].to_vec(),
            },
            Responses::FieldUpdate(f) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                let latest = f.latest;
                dat.push(latest.0);
                dat.push(latest.1);
                dat.push(match latest.2 {
                    Color::Black => 0,
                    Color::White => 1,
                });
                dat.extend(
                    compress_field(&f.field)
                        .iter()
                        .map(|x| [x.0, x.1, x.2, x.3])
                        .flatten(),
                );
                dat
            }
            Responses::Undo(f) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                let latest = f.latest;
                match latest {
                    None => dat.extend([0u8; 4]),
                    Some(latest) => {
                        dat.push(1);
                        dat.push(latest.0);
                        dat.push(latest.1);
                        dat.push(match latest.2 {
                            Color::Black => 0,
                            Color::White => 1,
                        });
                    }
                }
                dat.extend(
                    compress_field(&f.field)
                        .iter()
                        .map(|x| [x.0, x.1, x.2, x.3])
                        .flatten(),
                );
                dat
            }
            Responses::OpponentJoinRoom(name) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                dat.extend(name.as_bytes());
                dat
            }
            Responses::GameSessionError(e) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                dat.extend(e.as_bytes());
                dat
            }
            Responses::ChatMessage(name, msg) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                dat.extend((name.len() as u16).to_be_bytes());
                dat.extend(name.as_bytes());
                dat.extend(msg.as_bytes());
                dat
            }
            Responses::RoomScores((p0_name, p0), (p1_name, p1)) => {
                let mut dat = Vec::new();
                dat.push(self.response_type());
                dat.extend((p0_name.len() as u16).to_be_bytes());
                dat.extend(p0_name.as_bytes());
                dat.extend(p0.to_be_bytes());
                dat.extend((p1_name.len() as u16).to_be_bytes());
                dat.extend(p1_name.as_bytes());
                dat.extend(p1.to_be_bytes());
                dat
            }
            Responses::FromPlayer(name, msg) => {
                let mut dat = Vec::with_capacity(3 + name.len() + msg.len());
                dat.push(self.response_type());
                dat.extend((name.len() as u16).to_be_bytes());
                dat.extend(name.as_bytes());
                dat.extend(msg);
                dat
            }
        }
    }
}

impl TryFrom<Vec<u8>> for Responses {
    type Error = anyhow::Error;

    #[unroll_for_loops]
    fn try_from(bytes: Vec<u8>) -> Result<Self> {
        match bytes[0] {
            0 => Ok(Responses::RoomCreated(decode_utf8_string(&bytes)?)),
            2 => Ok(Responses::JoinRoomFailureTokenNotFound),
            3 => Ok(Responses::JoinRoomFailureRoomFull),
            4 => Ok(Responses::OpponentJoinRoom(decode_utf8_string(&bytes)?)),
            5 => Ok(Responses::OpponentQuitRoom),
            6 => Ok(Responses::OpponentReady),
            7 => Ok(Responses::OpponentUnready),
            10 => Ok(Responses::UndoRequest),
            11 => Ok(Responses::UndoTimeoutRejected),
            12 => Ok(Responses::UndoAutoRejected),
            14 => Ok(Responses::UndoRejectedByOpponent),
            15 => Ok(Responses::GameEndBlackTimeout),
            16 => Ok(Responses::GameEndWhiteTimeout),
            17 => Ok(Responses::GameEndBlackWins),
            18 => Ok(Responses::GameEndWhiteWins),
            19 => Ok(Responses::GameEndDraw),
            20 => Ok(Responses::OpponentQuitGameSession),
            21 => Ok(Responses::OpponentExitGame),
            22 => Ok(Responses::OpponentDisconnected),
            100 => Ok(Responses::ConnectionSuccess),
            1 => {
                if bytes.len() < 3 {
                    Err(Error::msg(
                        "server response decode error, incorrect byte length",
                    ))?
                }
                let token_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
                if bytes.len() < 3 + token_len + 1 {
                    Err(Error::msg(
                        "server response decode error, incorrect byte length",
                    ))?
                }
                let token = String::from_utf8(bytes[3..3 + token_len].to_vec())
                    .map_err(|_| Error::msg("utf-8 decode error"))?;
                match bytes[3 + token_len] {
                    0 => Ok(Responses::JoinRoomSuccess(token, RoomState::Empty)),
                    1 => Ok(Responses::JoinRoomSuccess(
                        token,
                        RoomState::OpponentUnready(
                            String::from_utf8(bytes[4 + token_len..].to_vec())
                                .map_err(|_| Error::msg("utf-8 decode error"))?,
                        ),
                    )),
                    2 => Ok(Responses::JoinRoomSuccess(
                        token,
                        RoomState::OpponentReady(
                            String::from_utf8(bytes[4 + token_len..].to_vec())
                                .map_err(|_| Error::msg("utf-8 decode error"))?,
                        ),
                    )),
                    _ => Err(Error::msg("unknown room state")),
                }
            }
            110 => {
                if bytes.len() != 2 {
                    Err(Error::msg(
                        "server response decode error, incorrect byte length",
                    ))?
                }
                Ok(Responses::ConnectionInitFailure(match bytes[1] {
                    1 => ConnectionInitError::IpMaxConnExceed,
                    2 => ConnectionInitError::ConnectionClosed,
                    3 => ConnectionInitError::UserNameNotReceived,
                    4 => ConnectionInitError::UserNameTooLong,
                    5 => ConnectionInitError::UserNameExists,
                    6 => ConnectionInitError::InvalidUserName,
                    _ => ConnectionInitError::NetworkError(ConnectionError::UnknownError),
                }))
            }
            120 => {
                if bytes.len() < 3 {
                    Err(Error::msg(
                        "client message decode error, incorrect byte length",
                    ))?
                }
                let name_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
                if bytes.len() < 3 + name_len {
                    Err(Error::msg(
                        "client message decode error, incorrect byte length",
                    ))?
                }
                let name = String::from_utf8(bytes[3..(3 + name_len)].to_vec())
                    .map_err(|_| Error::msg("utf-8 decode error"))?;
                let msg = bytes[(3 + name_len)..].to_vec();
                Ok(Responses::FromPlayer(name, msg))
            }
            200 => {
                let (name, msg) = decode_chat_message(&bytes)?;
                Ok(Responses::ChatMessage(name, msg))
            }
            8 => {
                if bytes.len() != 2 {
                    Err(Error::msg(
                        "server response decode error, incorrect byte length",
                    ))?
                }
                let color = match bytes[1] {
                    0 => Black,
                    1 => White,
                    _ => Err(Error::msg(
                        "server response decode error, incorrect color byte",
                    ))?,
                };
                Ok(Responses::GameStarted(color))
            }
            9 => {
                if bytes.len() != 1 + 3 + 4 * 15 {
                    Err(Error::msg(
                        "server response decode error, incorrect byte length",
                    ))?
                }
                let x = bytes[1];
                let y = bytes[2];
                let color = match bytes[3] {
                    0 => Black,
                    1 => White,
                    _ => Err(Error::msg(
                        "server response decode error, incorrect color byte",
                    ))?,
                };
                let mut field_dat = [(0u8, 0u8, 0u8, 0u8); 15];
                for i in 0..15 {
                    field_dat[i] = (
                        bytes[4 + 4 * i],
                        bytes[5 + 4 * i],
                        bytes[6 + 4 * i],
                        bytes[7 + 4 * i],
                    )
                }
                let field_state = FieldState {
                    latest: (x, y, color),
                    field: decompress_field(&field_dat),
                };
                Ok(Responses::FieldUpdate(field_state))
            }
            13 => {
                if bytes.len() != 1 + 4 + 4 * 15 {
                    Err(Error::msg(
                        "server response decode error, incorrect byte length",
                    ))?
                }
                let latest = match bytes[1] {
                    0 => None,
                    1 => {
                        let x = bytes[2];
                        let y = bytes[3];
                        let color = match bytes[4] {
                            0 => Black,
                            1 => White,
                            _ => Err(Error::msg(
                                "server response decode error, incorrect color byte",
                            ))?,
                        };
                        Some((x, y, color))
                    }
                    _ => Err(Error::msg(
                        "server response decode error, incorrect option byte",
                    ))?,
                };
                let mut field_dat = [(0u8, 0u8, 0u8, 0u8); 15];
                for i in 0..15 {
                    field_dat[i] = (
                        bytes[5 + 4 * i],
                        bytes[6 + 4 * i],
                        bytes[7 + 4 * i],
                        bytes[8 + 4 * i],
                    )
                }
                let field_state = FieldStateNullable {
                    latest,
                    field: decompress_field(&field_dat),
                };
                Ok(Responses::Undo(field_state))
            }
            23 => {
                let error_message = decode_utf8_string(&bytes)?;
                Ok(Responses::GameSessionError(error_message))
            }
            24 => {
                let (p0, p1) = decode_scores(&bytes)?;
                Ok(Responses::RoomScores(p0, p1))
            }
            _ => Err(Error::msg("server response decode error")),
        }
    }
}

fn decode_chat_message(bytes: &[u8]) -> Result<(String, String)> {
    if bytes.len() < 3 {
        return Err(Error::msg("incorrect byte length"));
    }
    let name_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
    if bytes.len() < 3 + name_len {
        return Err(Error::msg("incorrect byte length"));
    }
    let name = String::from_utf8(bytes[3..(3 + name_len)].to_vec())
        .map_err(|_| Error::msg("utf-8 decode error"))?;
    let chat_message = String::from_utf8(bytes[(3 + name_len)..].to_vec())
        .map_err(|_| Error::msg("utf-8 decode error"))?;
    Ok((name, chat_message))
}

// This function will take care of the first message type byte
fn decode_scores(bytes: &[u8]) -> Result<((String, u16), (String, u16))> {
    if bytes.len() < 3 {
        return Err(Error::msg("incorrect byte length"));
    }
    let p0_name_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
    if bytes.len() < 7 + p0_name_len {
        return Err(Error::msg("incorrect byte length"));
    }
    let p0_name = String::from_utf8(bytes[3..(3 + p0_name_len)].to_vec())
        .map_err(|_| Error::msg("utf-8 decode error"))?;
    let p0_score = u16::from_be_bytes([bytes[3 + p0_name_len], bytes[4 + p0_name_len]]);
    let p1_name_len = u16::from_be_bytes([bytes[5 + p0_name_len], bytes[6 + p0_name_len]]) as usize;
    if bytes.len() < 9 + p0_name_len + p1_name_len {
        return Err(Error::msg("incorrect byte length"));
    }
    let p1_name =
        String::from_utf8(bytes[(7 + p0_name_len)..(7 + p0_name_len + p1_name_len)].to_vec())
            .map_err(|_| Error::msg("utf-8 decode error"))?;
    let p1_score = u16::from_be_bytes([
        bytes[7 + p0_name_len + p1_name_len],
        bytes[8 + p0_name_len + p1_name_len],
    ]);
    Ok(((p0_name, p0_score), (p1_name, p1_score)))
}

/// This function will take care of the first message type byte
fn decode_utf8_string(bytes: &[u8]) -> Result<String> {
    let bytes_len = bytes.len();
    if bytes_len < 2 {
        Err(Error::msg(
            "server response decode error, incorrect byte length",
        ))?
    }
    String::from_utf8(bytes[1..].to_vec()).map_err(|_| Error::msg("utf-8 decode error"))
}

#[cfg(test)]
mod test_encode_decode {
    use super::*;
    use crate::game::State;
    use rand::thread_rng;

    fn assert_msg_eq(msg: Messages) {
        let decoded_msg =
            Messages::try_from(<Messages as Into<Vec<u8>>>::into(msg.clone())).unwrap();
        assert_eq!(msg, decoded_msg)
    }

    fn assert_rsp_eq(rsp: Responses) {
        let decoded_rsp =
            Responses::try_from(<Responses as Into<Vec<u8>>>::into(rsp.clone())).unwrap();
        assert_eq!(rsp, decoded_rsp)
    }

    #[test]
    fn test_messages() {
        let mut rng = thread_rng();
        assert_msg_eq(Messages::CreateRoom(SessionConfig {
            undo_request_timeout: 1,
            undo_dialogue_extra_seconds: 2,
            play_timeout: 3,
        }));
        assert_msg_eq(Messages::JoinRoom(RoomToken::random(&mut rng)));
        assert_msg_eq(Messages::QuitRoom);
        assert_msg_eq(Messages::Ready);
        assert_msg_eq(Messages::Unready);
        assert_msg_eq(Messages::Play(5, 3));
        assert_msg_eq(Messages::RequestUndo);
        assert_msg_eq(Messages::ApproveUndo);
        assert_msg_eq(Messages::RejectUndo);
        assert_msg_eq(Messages::QuitGameSession);
        assert_msg_eq(Messages::ExitGame);
        assert_msg_eq(Messages::ClientError("decode error".to_string()));
        assert_msg_eq(Messages::UserName("user name".to_string()));
        assert_msg_eq(Messages::ChatMessage("chat message".to_string()));
        assert_msg_eq(Messages::ToPlayer("香菱".to_string(), Vec::from("good")));
        assert_msg_eq(Messages::ToPlayer("香菱".to_string(), Vec::new()));
    }

    #[test]
    fn test_responses() {
        let mut rng = thread_rng();
        assert_rsp_eq(Responses::RoomCreated(
            RoomToken::random(&mut rng).as_code(),
        ));
        assert_rsp_eq(Responses::ConnectionSuccess);
        assert_rsp_eq(Responses::JoinRoomSuccess(
            RoomToken::random(&mut rng).as_code(),
            RoomState::OpponentReady("枫原万叶".to_string()),
        ));
        assert_rsp_eq(Responses::JoinRoomFailureTokenNotFound);
        assert_rsp_eq(Responses::JoinRoomFailureRoomFull);
        assert_rsp_eq(Responses::OpponentJoinRoom("some username".to_string()));
        assert_rsp_eq(Responses::OpponentQuitRoom);
        assert_rsp_eq(Responses::OpponentReady);
        assert_rsp_eq(Responses::OpponentUnready);
        assert_rsp_eq(Responses::GameStarted(Color::Black));
        assert_rsp_eq(Responses::FieldUpdate(FieldState {
            latest: (5, 3, Color::Black),
            field: [[State::B; 15]; 15],
        }));
        assert_rsp_eq(Responses::UndoRequest);
        assert_rsp_eq(Responses::UndoTimeoutRejected);
        assert_rsp_eq(Responses::UndoAutoRejected);
        assert_rsp_eq(Responses::Undo(FieldStateNullable {
            latest: None,
            field: [[State::W; 15]; 15],
        }));
        assert_rsp_eq(Responses::Undo(FieldStateNullable {
            latest: Some((5, 3, Color::White)),
            field: [[State::E; 15]; 15],
        }));
        assert_rsp_eq(Responses::UndoRejectedByOpponent);
        assert_rsp_eq(Responses::GameEndBlackTimeout);
        assert_rsp_eq(Responses::GameEndWhiteTimeout);
        assert_rsp_eq(Responses::GameEndBlackWins);
        assert_rsp_eq(Responses::GameEndWhiteWins);
        assert_rsp_eq(Responses::GameEndDraw);
        assert_rsp_eq(Responses::OpponentQuitGameSession);
        assert_rsp_eq(Responses::OpponentExitGame);
        assert_rsp_eq(Responses::OpponentDisconnected);
        assert_rsp_eq(Responses::RoomScores(
            ("枫原万叶".to_string(), 5),
            ("巴巴托斯".to_string(), 3),
        ));
        assert_rsp_eq(Responses::ChatMessage(
            "神里绫华".to_string(),
            "hi!".to_string(),
        ));
        assert_rsp_eq(Responses::GameSessionError("some error".to_string()));
        assert_rsp_eq(Responses::ConnectionInitFailure(
            ConnectionInitError::UserNameTooLong,
        ));
        assert_rsp_eq(Responses::FromPlayer("香菱".to_string(), Vec::from("good")));
        assert_rsp_eq(Responses::FromPlayer("香菱".to_string(), Vec::new()));
    }
}
