use crate::game::game_field::field::{Field, GameState};
use crate::game::game_field::Color;
use crate::game::session::{FieldState, FieldStateNullable};
use crate::CHANNEL_SIZE;
use anyhow::{Error, Result};
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use futures::StreamExt;
use log::{error, trace};
use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) enum GameCommand {
    Do { x: u8, y: u8, color: Color },
    Undo,
    Kill,
}

#[derive(Debug)]
pub(crate) enum GameResponse {
    /// color in field indicates the color of the latest step
    Field(FieldState),
    BlackWins,
    WhiteWins,
    Draw,
    Undo(FieldStateNullable),
    GameError(String),
}

/// Start a new field.
///
/// This actor stops when it is gets a `Kill` signal or when its response receiver
/// gets dropped.
pub(crate) fn new_field(session_id: u64) -> (Sender<GameCommand>, Receiver<GameResponse>) {
    let (cmd_s, mut commands) = bounded(CHANNEL_SIZE);
    let (response, rsp_r) = bounded(CHANNEL_SIZE);
    let mut history = VecDeque::with_capacity(225);
    let mut field = Field::new();
    task::spawn(async move {
        while let Some(command) = commands.next().await {
            #[cfg(debug_assertions)]
            trace!(
                "field of game {} received command {:?}",
                session_id,
                command
            );
            if execute_command(session_id, &mut field, command, &response, &mut history)
                .await
                .is_err()
            {
                #[cfg(debug_assertions)]
                trace!("field thread of game {} stopped on err", session_id);
                break;
            }
        }
        #[cfg(debug_assertions)]
        trace!("field thread of game {} stopped", session_id);
    });
    (cmd_s, rsp_r)
}

/// the error of this function means game killed or receivers dropped, just exit
async fn execute_command(
    game_id: u64,
    field: &mut Field,
    command: GameCommand,
    response: &Sender<GameResponse>,
    history: &mut VecDeque<(u8, u8, Color)>,
) -> Result<()> {
    match command {
        GameCommand::Do { x, y, color } => {
            do_play(game_id, field, x, y, color, history, response).await
        }
        GameCommand::Undo => undo_play(game_id, field, history, response).await,
        GameCommand::Kill => Err(Error::msg("game killed")),
    }
}

async fn do_play(
    game_id: u64,
    field: &mut Field,
    x: u8,
    y: u8,
    color: Color,
    history: &mut VecDeque<(u8, u8, Color)>,
    response: &Sender<GameResponse>,
) -> Result<()> {
    if let Err(e) = field.play(x as usize, y as usize, color) {
        send_unlikely_error(e, game_id, response).await
    } else {
        history.push_back((x, y, color));
        send_game_state(x, y, color, field, response).await
    }
}

/// the error of this function can only come from being receivers being closed, just exit
async fn undo_play(
    game_id: u64,
    field: &mut Field,
    history: &mut VecDeque<(u8, u8, Color)>,
    response: &Sender<GameResponse>,
) -> Result<()> {
    if let Some((x, y, _)) = history.pop_back() {
        if let Err(e) = field.clear(x as usize, y as usize) {
            send_unlikely_error(e, game_id, response).await
        } else {
            let prev = history.back().map(|&(x, y, c)| (x, y, c));
            send_undo_state(prev, field, response).await
        }
    } else {
        Ok(())
    }
}

#[inline(always)]
async fn send_game_state(
    x: u8,
    y: u8,
    color: Color,
    field: &Field,
    response: &Sender<GameResponse>,
) -> Result<()> {
    // send field update
    response
        .send(GameResponse::Field(FieldState {
            latest: (x, y, color),
            field: *field.get_field(),
        }))
        .await?;
    // send field state
    match field.get_field_state() {
        GameState::BlackWins => response.send(GameResponse::BlackWins).await?,
        GameState::WhiteWins => response.send(GameResponse::WhiteWins).await?,
        GameState::Draw => response.send(GameResponse::Draw).await?,
        GameState::Impossible => {
            response
                .send(GameResponse::GameError("impossible_game_state".to_string()))
                .await?
        }
        GameState::UnFinished => {}
    }
    Ok(())
}

#[inline(always)]
async fn send_undo_state(
    prev: Option<(u8, u8, Color)>,
    field: &Field,
    response: &Sender<GameResponse>,
) -> Result<()> {
    Ok(response
        .send(GameResponse::Undo(FieldStateNullable {
            latest: prev,
            field: *field.get_field(),
        }))
        .await?)
}

/// the error of this function can only come from being receivers being closed, just exit
#[cold]
async fn send_unlikely_error(
    e: Error,
    game_id: u64,
    response: &Sender<GameResponse>,
) -> Result<()> {
    error!("game no {} error: {}", game_id, e);
    Ok(response
        .send(GameResponse::GameError(e.to_string()))
        .await?)
}
