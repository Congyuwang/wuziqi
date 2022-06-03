use crate::game::game_field::Color::{Black, White};
use crate::game::game_field::{Color, GameCommand, GameResponse, State};
use crate::game::session::{
    FieldState, FieldStateNullable, GameQuitResponse, GameResult, PlayerQuitReason, UndoResponse,
};
use crate::CHANNEL_SIZE;
use anyhow::Result;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use futures::{stream_select, StreamExt};
use std::fmt::{Formatter, Write};

/// actions received from players
#[derive(Debug)]
pub(crate) enum PlayerAction {
    Play(u8, u8),
    RequestUndo,
    Undo(UndoAction),
    /// player sends this if it needs to quit
    Quit(PlayerQuitReason),
}

/// player actions
#[derive(Debug)]
pub(crate) enum UndoAction {
    Approve,
    Reject,
}

/// actions received from players
#[derive(Debug)]
pub(crate) enum SessionPlayerAction {
    Play(u8, u8),
    PlayTimeout,
    RequestUndo,
    Undo(SessionUndoAction),
    /// player sends this if it needs to quit
    Quit(PlayerQuitReason),
}

/// response to players
#[derive(Clone, Debug)]
pub(crate) enum SessionPlayerResponse {
    FieldUpdate(FieldState),
    UndoRequest,
    Undo(UndoResponse),
    /// game end, player quit, error, and etc,
    Quit(GameQuitResponse),
}

/// player actions
#[derive(Debug)]
pub(crate) enum SessionUndoAction {
    Approve,
    Reject,
    AutoReject,
    TimeoutReject,
}

/// messages sent to the session from players or game
#[derive(Debug)]
pub(crate) enum SessionMessage {
    Player(Color, SessionPlayerAction),
    Game(GameResponse),
    Kill(GameQuitResponse),
}

/// response sent from the session to players ot game
#[derive(Debug)]
pub(crate) enum SessionResponse {
    Player(Color, SessionPlayerResponse),
    Game(GameCommand),
}

pub(crate) struct SessionKiller(Sender<SessionMessage>);

impl SessionKiller {
    pub(crate) async fn kill(&self, q: GameQuitResponse) -> Result<()> {
        Ok(self.0.send(SessionMessage::Kill(q)).await?)
    }
}

/// This is a router tha distribute all messages to game, black player, and white player.
///
/// Message sender stops when session ends or when all players are dropped.
pub(crate) fn message_sender(
    black: Sender<SessionPlayerResponse>,
    white: Sender<SessionPlayerResponse>,
    game: Sender<GameCommand>,
) -> Sender<SessionResponse> {
    let (sender, mut receiver) = bounded(CHANNEL_SIZE);
    task::spawn(async move {
        while let Some(session_response) = receiver.next().await {
            if match session_response {
                SessionResponse::Player(color, action) => match color {
                    Black => black.send(action).await.is_err(),
                    White => white.send(action).await.is_err(),
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
    black: Receiver<SessionPlayerAction>,
    white: Receiver<SessionPlayerAction>,
    game: Receiver<GameResponse>,
) -> (SessionKiller, Receiver<SessionMessage>) {
    let (message_sender, messages) = bounded(CHANNEL_SIZE);
    let killer = message_sender.clone();
    task::spawn(async move {
        let black = black.map(|act| SessionMessage::Player(Black, act)).fuse();
        let white = white.map(|act| SessionMessage::Player(White, act)).fuse();
        let game = game.map(SessionMessage::Game).fuse();
        let mut fused = stream_select!(black, white, game);
        while let Some(message) = fused.next().await {
            if message_sender.send(message).await.is_err() {
                break;
            }
        }
    });
    (SessionKiller(killer), messages)
}

pub(crate) async fn broadcast_to_players(
    player_response: SessionPlayerResponse,
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

impl std::fmt::Display for GameResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GameResult::BlackWins => f.write_str("BlackWins"),
            GameResult::WhiteWins => f.write_str("WhiteWins"),
            GameResult::Draw => f.write_str("Draw"),
            GameResult::BlackTimeout => f.write_str("BlackTimeout"),
            GameResult::WhiteTimeout => f.write_str("WhiteTimeout"),
        }
    }
}

impl std::fmt::Debug for FieldState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let (x, y, _) = self.latest;
        write_field_with_latest(x, y, &self.field, f)?;
        Ok(())
    }
}

impl std::fmt::Debug for FieldStateNullable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.latest {
            None => write_empty_field(f)?,
            Some((x, y, _)) => write_field_with_latest(x, y, &self.field, f)?,
        }
        Ok(())
    }
}

fn write_empty_field(f: &mut Formatter<'_>) -> std::fmt::Result {
    for _ in 0..15 {
        for _ in 0..15 {
            f.write_str(".  ")?;
        }
        f.write_char('\n')?;
    }
    Ok(())
}

fn write_field_with_latest(
    x: u8,
    y: u8,
    field: &[[State; 15]; 15],
    f: &mut Formatter<'_>,
) -> std::fmt::Result {
    for (i, row) in field.iter().enumerate() {
        for (j, s) in row.iter().enumerate() {
            if i == x as usize && j == y as usize {
                match s {
                    State::B => f.write_char('X')?,
                    State::W => f.write_char('O')?,
                    State::E => panic!("latest position could not be empty"),
                }
            } else {
                match s {
                    State::B => f.write_char('x')?,
                    State::W => f.write_char('o')?,
                    State::E => f.write_char('.')?,
                }
            }
            f.write_char(' ')?;
            f.write_char(' ')?;
        }
        f.write_char('\n')?;
    }
    Ok(())
}
