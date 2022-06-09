pub mod game;
pub mod lobby;
pub(crate) mod network;
mod stream_utility;

pub use game::*;
pub use lobby::{
    start_server, ConnectionInitError, CreateAccountFailure, InvalidAccountPassword, LoginFailure,
    Messages, Responses, RoomState, RoomToken, UpdatePasswordFailure,
};
pub use network::{Conn, ConnectionError, Received};

pub(crate) const CHANNEL_SIZE: usize = 5;
