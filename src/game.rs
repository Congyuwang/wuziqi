use crate::field::{Field, FieldState, State};
use anyhow::{Error, Result};
use async_std::channel::{Receiver, SendError, Sender};
use futures::StreamExt;
use log::{error, info};
use std::collections::VecDeque;

pub enum GameCommand {
    Do { x: u8, y: u8, state: State },
    Undo,
    Kill,
}

pub enum GameResponse {
    Field([[State; 15]; 15]),
    BlackWins,
    WhiteWins,
    Draw,
    GameError(String),
    GameStopped,
}

pub async fn new_game(
    game_id: usize,
    mut commands: Receiver<GameCommand>,
    mut response: Sender<GameResponse>,
) {
    let mut history = VecDeque::new();
    let mut field = Field::new();
    while let Some(command) = commands.next().await {
        if let Err(e) =
            execute_command(game_id, &mut field, &command, &response, &mut history).await
        {
            info!("game no {} stopped: {}", game_id, e);
            let _ = response.send(GameResponse::GameStopped).await;
            break;
        }
    }
}

/// the error of this function means game killed or receivers dropped, just exit
pub async fn execute_command(
    game_id: usize,
    field: &mut Field,
    command: &GameCommand,
    response: &Sender<GameResponse>,
    history: &mut VecDeque<(u8, u8, State)>,
) -> Result<()> {
    match &command {
        GameCommand::Do { x, y, state } => {
            do_play(game_id, field, x, y, state, &response, history, true).await
        }
        GameCommand::Undo => undo_play(game_id, field, history, &response).await,
        GameCommand::Kill => Err(Error::msg("game killed")),
    }
}

/// the error of this function can only come from being receivers being closed, just exit
async fn do_play(
    game_id: usize,
    field: &mut Field,
    x: &u8,
    y: &u8,
    state: &State,
    response: &Sender<GameResponse>,
    history: &mut VecDeque<(u8, u8, State)>,
    if_record: bool,
) -> Result<()> {
    if let Err(e) = field.play(*x as usize, *y as usize, *state) {
        // send field play error
        error!("game no {} error: {}", game_id, e);
        response
            .send(GameResponse::GameError(e.to_string()))
            .await?;
    } else {
        // send field update
        response
            .send(GameResponse::Field(field.get_field().clone()))
            .await?;
        // send field state
        match field.get_field_state() {
            FieldState::BlackWins => response.send(GameResponse::BlackWins).await?,
            FieldState::WhiteWins => response.send(GameResponse::WhiteWins).await?,
            FieldState::Draw => response.send(GameResponse::Draw).await?,
            FieldState::Impossible => {
                response
                    .send(GameResponse::GameError("impossible_game_state".to_string()))
                    .await?
            }
            _ => {}
        }
        // record commands
        if if_record {
            history.push_front((*x, *y, *state));
        }
    }
    Ok(())
}

/// the error of this function can only come from being receivers being closed, just exit
pub async fn undo_play(
    game_id: usize,
    field: &mut Field,
    history: &mut VecDeque<(u8, u8, State)>,
    response: &Sender<GameResponse>,
) -> Result<()> {
    if let Some((x, y, _)) = history.pop_back() {
        do_play(game_id, field, &x, &y, &State::E, &response, history, false).await?
    }
    Ok(())
}
