use crate::game::game_field::{Color, State};
use crate::game::session::messages::{PlayerAction, UndoAction};
use anyhow::Result;
use async_std::channel::{Receiver, Sender};

/// Public API used for interacting with the game
pub struct Player {
    listener: Option<Receiver<PlayerResponse>>,
    action_sender: Sender<PlayerAction>,
}

/// all player actions are here
impl Player {
    /// play a certain step
    pub async fn play(&self, x: u8, y: u8) -> Result<()> {
        self.action_sender.send(PlayerAction::Play(x, y)).await?;
        Ok(())
    }

    pub async fn request_undo(&self) -> Result<()> {
        self.action_sender.send(PlayerAction::RequestUndo).await?;
        Ok(())
    }

    pub async fn approve_undo(&self) -> Result<()> {
        self.action_sender
            .send(PlayerAction::Undo(UndoAction::Approve))
            .await?;
        Ok(())
    }

    pub async fn reject_undo(&self) -> Result<()> {
        self.action_sender
            .send(PlayerAction::Undo(UndoAction::Reject))
            .await?;
        Ok(())
    }

    pub async fn quit(&self, reason: PlayerQuitReason) -> Result<()> {
        self.action_sender.send(PlayerAction::Quit(reason)).await?;
        Ok(())
    }

    pub fn get_listener(&mut self) -> Option<Receiver<PlayerResponse>> {
        self.listener.take()
    }

    pub(crate) fn new(
        action_sender: Sender<PlayerAction>,
        listener: Receiver<PlayerResponse>,
    ) -> Player {
        Player {
            listener: Some(listener),
            action_sender,
        }
    }
}

/// the reason of player quit
#[derive(Debug)]
pub enum PlayerQuitReason {
    Quit,
    Disconnected,
    Error(String),
}

/// response to players
#[derive(Clone, Debug)]
pub enum PlayerResponse {
    FieldUpdate(FieldState),
    UndoRequest,
    Undo(UndoResponse),
    /// other player quit or game error
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
    GameEnd(GameResult),
    PlayerQuit(u64),
    PlayerDisconnected(u64),
    PlayerError(u64, String),
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
#[derive(Clone)]
pub struct FieldState {
    pub latest: (u8, u8, Color),
    pub field: [[State; 15]; 15],
}

/// this struct represents a game field
/// and also the coordinate of the latest position
#[derive(Clone)]
pub struct FieldStateNullable {
    pub latest: Option<(u8, u8, Color)>,
    pub field: [[State; 15]; 15],
}

/// client time should be shorter
///
/// 0 means no timeout
#[derive(Clone)]
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
