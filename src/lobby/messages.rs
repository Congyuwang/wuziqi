//! Implementation principles.
//! - disconnection without clear exit signal is considered as disconnection.
use crate::game::{Color, FieldState, FieldStateNullable, SessionConfig};
use crate::lobby::client_connection::ConnectionInitError;
use crate::lobby::token::RoomToken;
use anyhow::Error;
use bincode::config::Configuration;
use bincode::{config, Decode, Encode};
use bincode::{decode_from_slice, encode_to_vec};

const BIN_CONFIG: Configuration = config::standard().with_variable_int_encoding();

#[derive(Clone, PartialEq, Debug, Encode, Decode)]
pub enum Messages {
    ToPlayer(String, Vec<u8>),
    /// create an account with username and password
    CreateAccount(String, String),
    /// login with username and password
    Login(String, String),
    /// update password (username, old password, new password)
    UpdateAccount(String, String, String),
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
    /// search online player names (name, limit)
    /// currently, at most 20 names will be returned
    SearchOnlinePlayers(Option<String>, u8),
    /// chat message
    ChatMessage(String),
    /// exit game (quit game and room), close connection
    /// exiting game without sending `ExitGame` signal is considered `Disconnected`
    ExitGame,
    /// client error: other errors excluding network error
    ClientError(String),
}

#[derive(Clone, PartialEq, Debug, Encode, Decode)]
pub enum RoomState {
    Empty,
    OpponentReady(String),
    OpponentUnready(String),
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum InvalidAccountPassword {
    BadCharacterAccountName,
    AccountNameTooShort,
    AccountNameTooLong,
    BadCharacterAccountPassword,
    PasswordTooShort,
    PasswordTooLong,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum CreateAccountFailure {
    BadInput(InvalidAccountPassword),
    AccountAlreadyExist,
    ServerError,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum UpdatePasswordFailure {
    BadInput(InvalidAccountPassword),
    UserDoesNotExist,
    PasswordIncorrect,
    ServerError,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum LoginFailure {
    BadInput(InvalidAccountPassword),
    AccountDoesNotExist,
    PasswordIncorrect,
    ServerError,
}

#[derive(Clone, PartialEq, Debug, Encode, Decode)]
pub enum Responses {
    FromPlayer(String, Vec<u8>),
    /// create account failure with reason
    CreateAccountFailure(CreateAccountFailure),
    /// login failure with reason
    LoginFailure(LoginFailure),
    /// update account failure
    UpdateAccountFailure(UpdatePasswordFailure),
    /// create account success with username and password
    CreateAccountSuccess(String, String),
    /// update password success
    UpdateAccountSuccess(String, String),
    /// login success with username
    LoginSuccess(String),
    /// Connection Init Error
    ConnectionInitFailure(ConnectionInitError),
    /// response to `CreateRoom`
    RoomCreated(String),
    /// response to `SearchOnlinePlayers`
    PlayerList(Vec<String>),
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

impl Into<Vec<u8>> for Messages {
    fn into(self) -> Vec<u8> {
        encode_to_vec(self, BIN_CONFIG).unwrap()
    }
}

impl Into<Vec<u8>> for Responses {
    fn into(self) -> Vec<u8> {
        encode_to_vec(self, BIN_CONFIG).unwrap()
    }
}

impl TryFrom<Vec<u8>> for Messages {
    type Error = Error;

    fn try_from(value: Vec<u8>) -> std::result::Result<Self, Self::Error> {
        match decode_from_slice(&value, BIN_CONFIG) {
            Ok((msg, _)) => Ok(msg),
            Err(_) => Err(Error::msg("client message decode error".to_string())),
        }
    }
}

impl TryFrom<Vec<u8>> for Responses {
    type Error = Error;

    fn try_from(value: Vec<u8>) -> std::result::Result<Self, Self::Error> {
        match decode_from_slice(&value, BIN_CONFIG) {
            Ok((msg, _)) => Ok(msg),
            Err(_) => Err(Error::msg("server response decode error".to_string())),
        }
    }
}

#[cfg(test)]
mod test_encode_decode {
    use super::*;
    use crate::game::State;
    use crate::FieldInner;
    use rand::thread_rng;
    use crate::Color::{Black, White};

    fn assert_msg_eq(msg: Messages) {
        let decoded_msg =
            Messages::try_from(<Messages as Into<Vec<u8>>>::into(msg.clone())).unwrap();
        assert_eq!(msg, decoded_msg)
    }

    fn assert_rsp_eq(rsp: Responses) {
        let decoded_rsp = Responses::try_from(<Responses as Into<Vec<u8>>>::into(rsp.clone()));
        match decoded_rsp {
            Ok(decoded_rsp) => assert_eq!(rsp, decoded_rsp),
            Err(e) => {
                println!("failed on error: {}", e);
                panic!()
            }
        }
    }

    #[test]
    fn test_messages() {
        let mut rng = thread_rng();
        assert_msg_eq(Messages::CreateRoom(SessionConfig {
            undo_request_timeout: 1,
            undo_dialogue_extra_seconds: 2,
            play_timeout: 3,
        }));
        assert_msg_eq(Messages::Login("小雨".to_string(), "okk".to_string()));
        assert_msg_eq(Messages::CreateAccount(
            "雨雨".to_string(),
            "oh yeah".to_string(),
        ));
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
        assert_msg_eq(Messages::Login(
            "user name".to_string(),
            "password".to_string(),
        ));
        assert_msg_eq(Messages::CreateAccount(
            "user name".to_string(),
            "password".to_string(),
        ));
        assert_msg_eq(Messages::ChatMessage("chat message".to_string()));
        assert_msg_eq(Messages::ToPlayer("香菱".to_string(), Vec::from("good")));
        assert_msg_eq(Messages::ToPlayer("香菱".to_string(), Vec::new()));
        assert_msg_eq(Messages::SearchOnlinePlayers(None, 5));
        assert_msg_eq(Messages::SearchOnlinePlayers(Some("巴巴".to_string()), 5));
    }

    #[test]
    fn test_responses() {
        let mut rng = thread_rng();
        assert_rsp_eq(Responses::RoomCreated(
            RoomToken::random(&mut rng).as_code(),
        ));
        assert_rsp_eq(Responses::LoginSuccess("小雨".to_string()));
        assert_rsp_eq(Responses::LoginFailure(LoginFailure::AccountDoesNotExist));
        assert_rsp_eq(Responses::LoginSuccess("小雨".to_string()));
        assert_rsp_eq(Responses::CreateAccountFailure(
            CreateAccountFailure::BadInput(InvalidAccountPassword::BadCharacterAccountName),
        ));
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
        assert_rsp_eq(Responses::GameStarted(Black));
        assert_rsp_eq(Responses::FieldUpdate(FieldState {
            latest: (5, 3, Black),
            field: FieldInner([[State::B; 15]; 15]),
        }));
        assert_rsp_eq(Responses::UndoRequest);
        assert_rsp_eq(Responses::UndoTimeoutRejected);
        assert_rsp_eq(Responses::UndoAutoRejected);
        assert_rsp_eq(Responses::Undo(FieldStateNullable {
            latest: None,
            field: FieldInner([[State::W; 15]; 15]),
        }));
        assert_rsp_eq(Responses::Undo(FieldStateNullable {
            latest: Some((5, 3, White)),
            field: FieldInner([[State::E; 15]; 15]),
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
        assert_rsp_eq(Responses::PlayerList(vec![
            "枫原万叶".to_string(),
            "leon".to_string(),
            "神里绫华".to_string(),
        ]));
        assert_rsp_eq(Responses::PlayerList(vec![]));
    }
}
