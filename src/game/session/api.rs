use crate::game::game_field::{Color, State};
use crate::game::session::messages::{
    PlayerAction::{self, Play, Quit, RequestUndo, Undo},
    UndoAction::{Approve, Reject},
};
use async_std::channel::{Receiver, Sender};
use serde::{Serialize, Deserialize};

/// Public API used for interacting with the game
pub struct Commands {
    listener: Option<Receiver<PlayerResponse>>,
    action_sender: Sender<PlayerAction>,
}

/// all player actions are here
impl Commands {
    /// play a certain step
    pub async fn play(&self, x: u8, y: u8) {
        let _ = self.action_sender.send(Play(x, y)).await;
    }

    pub async fn request_undo(&self) {
        let _ = self.action_sender.send(RequestUndo).await;
    }

    pub async fn approve_undo(&self) {
        let _ = self.action_sender.send(Undo(Approve)).await;
    }

    pub async fn reject_undo(&self) {
        let _ = self.action_sender.send(Undo(Reject)).await;
    }

    /// `quit()` should be called before ending the game to properly
    /// notify the other player.
    ///
    /// Moreover, game session might stuck waiting for player message
    /// if the corresponding player `Commands` is dropped without
    /// calling `quit()` since game session wait for combined incoming
    /// messages.
    pub async fn quit(&self, reason: PlayerQuitReason) {
        let _ = self.action_sender.send(Quit(reason)).await;
    }

    pub fn get_listener(&mut self) -> Option<Receiver<PlayerResponse>> {
        self.listener.take()
    }

    pub(crate) fn new(
        action_sender: Sender<PlayerAction>,
        listener: Receiver<PlayerResponse>,
    ) -> Commands {
        Commands {
            listener: Some(listener),
            action_sender,
        }
    }
}

/// the reason of player quit
#[derive(Debug)]
pub enum PlayerQuitReason {
    QuitSession,
    ExitGame,
    Disconnected,
    Error(String),
}

/// response to players
#[derive(Clone, Debug)]
pub enum PlayerResponse {
    FieldUpdate(FieldState),
    UndoRequest,
    Undo(UndoResponse),
    /// Other player quit or game error.
    /// Game session will end automatically on
    /// receiving Quit response
    Quit(GameQuitResponse),
}

/// response to players
#[derive(Clone, Debug)]
pub enum UndoResponse {
    /// broadcast to both players
    TimeoutRejected,
    /// broadcast to both players
    Undo(FieldStateNullable),
    /// send only to requester
    RejectedByOpponent,
    /// send only to requester
    AutoRejected,
}

/// reason of game session end
#[derive(Clone, Debug)]
pub enum GameQuitResponse {
    /// broadcast to both players
    GameEnd(GameResult),
    /// send to opponent
    OpponentQuitSession(u64),
    /// send to opponent
    OpponentExitGame(u64),
    /// send to opponent
    OpponentDisconnected(u64),
    /// send to opponent
    OpponentError(u64, String),
    /// broadcast to both players
    GameError(String),
}

/// result of the game
#[derive(Clone, Debug)]
pub enum GameResult {
    BlackTimeout,
    WhiteTimeout,
    BlackWins,
    WhiteWins,
    Draw,
}

/// this struct represents a game field
/// and also the coordinate of the latest position
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldState {
    pub latest: (u8, u8, Color),
    pub field: [[State; 15]; 15],
}

/// this struct represents a game field
/// and also the coordinate of the latest position
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldStateNullable {
    pub latest: Option<(u8, u8, Color)>,
    pub field: [[State; 15]; 15],
}

/// client time should be shorter
///
/// 0 means no timeout
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SessionConfig {
    pub undo_request_timeout: u64,
    pub undo_dialogue_extra_seconds: u64,
    pub play_timeout: u64,
}

/// by default no restriction
impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            undo_request_timeout: 0,
            undo_dialogue_extra_seconds: 0,
            play_timeout: 0,
        }
    }
}
