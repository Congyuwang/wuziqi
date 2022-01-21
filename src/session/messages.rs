use crate::game::Color::{Black, White};
use crate::game::{Color, GameCommand, GameResponse, State};
use crate::session::messages::GameFinished::BlackWins;
use anyhow::Result;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use futures::{select, FutureExt, StreamExt, TryFutureExt};
use std::fmt::Formatter;

pub(crate) const CHANNEL_SIZE: usize = 20;

/// actions received from players
pub enum PlayerAction {
    Play(u8, u8),
    RequestUndo,
    Undo(UndoAction),
    /// player sends this if it needs to quit
    Quit(QuitReason),
}

/// response to players
#[derive(Clone)]
pub enum PlayerResponse {
    FieldUpdate(Color, [[State; 15]; 15]),
    UndoRequest,
    Undo(UndoResponse),
    /// other player quit or game error
    Quit(QuitResponse),
}

/// player actions
pub enum UndoAction {
    Approve,
    Reject,
    AutoReject,
    TimeOutReject,
}

/// the reason of player quit
pub enum QuitReason {
    PlayerQuit,
    PlayerDisconnected,
    PlayerError(String),
}

/// response to players
#[derive(Clone)]
pub enum UndoResponse {
    Undo([[State; 15]; 15]),
    RejectedByOpponent,
    AutoRejected,
    TimeOutRejected,
    NoMoreUndo,
}

/// information related to session end
#[derive(Clone)]
pub enum QuitResponse {
    GameEnd(GameFinished),
    PlayerQuit(u64),
    PlayerDisconnected(u64),
    PlayerError(u64, String),
    GameError(String),
}

/// response to players
#[derive(Clone)]
pub enum GameFinished {
    BlackWins,
    WhiteWins,
    Draw,
}

/// messages sent to the session from players or game
pub(crate) enum SessionMessage {
    Player(Color, PlayerAction),
    Game(GameResponse),
    Kill(QuitResponse),
}

/// response sent from the session to players ot game
pub(crate) enum SessionResponse {
    Player(Color, PlayerResponse),
    Game(GameCommand),
}

pub(crate) struct SessionKiller(Sender<SessionMessage>);

impl SessionKiller {
    pub(crate) async fn kill(&self, q: QuitResponse) -> Result<()> {
        Ok(self.0.send(SessionMessage::Kill(q)).await?)
    }
}

/// This is a router tha distribute all messages to game, black player, and white player.
///
/// Message sender stops when session ends or when all players are dropped.
pub(crate) fn message_sender(
    black: Sender<PlayerResponse>,
    white: Sender<PlayerResponse>,
    game: Sender<GameCommand>,
) -> Sender<SessionResponse> {
    let (sender, mut receiver) = bounded(CHANNEL_SIZE);
    task::spawn(async move {
        while let Some(session_response) = receiver.next().await {
            if match session_response {
                SessionResponse::Player(color, action) => match color {
                    Color::Black => black.send(action).await.is_err(),
                    Color::White => white.send(action).await.is_err(),
                },
                SessionResponse::Game(cmd) => game.send(cmd).await.is_err(),
            } {
                break;
            }
        }
    });
    sender
}

/// This is a router that collects all messages from game, black player, and white player.
///
/// Message sender stops when session ends or when all players are dropped.
pub(crate) fn message_receiver(
    black: Receiver<PlayerAction>,
    white: Receiver<PlayerAction>,
    game: Receiver<GameResponse>,
) -> (SessionKiller, Receiver<SessionMessage>) {
    let (message_sender, messages) = bounded(CHANNEL_SIZE);
    let killer = message_sender.clone();
    task::spawn(async move {
        let mut black = black.fuse();
        let mut white = white.fuse();
        let mut game = game.fuse();
        while let Some(message) = select! {
            black_action = black.next().fuse() => {
                match black_action {
                    Some(action) => Some(SessionMessage::Player(Black, action)),
                    None => None,
                }
            },
            white_action = white.next().fuse() => {
                match white_action {
                    Some(action) => Some(SessionMessage::Player(White, action)),
                    None => None,
                }
            },
            game_response = game.next().fuse() => {
                match game_response {
                    Some(response) => Some(SessionMessage::Game(response)),
                    None => None,
                }
            }
        } {
            if message_sender.send(message).await.is_err() {
                break;
            }
        }
    });
    (SessionKiller(killer), messages)
}

pub(crate) async fn broadcast_to_players(
    player_response: PlayerResponse,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    responses
        .send(SessionResponse::Player(Black, player_response.clone()))
        .await?;
    responses
        .send(SessionResponse::Player(White, player_response))
        .await?;
    Ok(())
}

impl std::fmt::Display for GameFinished {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GameFinished::BlackWins => f.write_str("BlackWins"),
            GameFinished::WhiteWins => f.write_str("WhiteWins"),
            GameFinished::Draw => f.write_str("Draw"),
        }
    }
}
