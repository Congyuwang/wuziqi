use crate::game::field::{Color, Field, GameState, State};
use anyhow::{Error, Result};
use async_std::channel::{Receiver, SendError, Sender};
use futures::StreamExt;
use log::{error, info};
use std::collections::VecDeque;
use crate::session::FieldState;

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
    Undo(FieldState),
    NoMoreUndo,
    GameError(String),
}

/// Start a new game.
///
/// This actor stops when it is gets a `Kill` signal or when its response receiver
/// gets dropped.
pub(crate) async fn new_game(
    game_id: u64,
    mut commands: Receiver<GameCommand>,
    mut response: Sender<GameResponse>,
) {
    let mut history = VecDeque::with_capacity(225);
    let mut field = Field::new();
    while let Some(command) = commands.next().await {
        if execute_command(game_id, &mut field, command, &response, &mut history)
            .await
            .is_err()
        {
            break;
        }
    }
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
        send_game_state(x, y, field, response).await
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
            let prev_step = history.back().map(|&(x, y, _)| (x, y));
            send_undo_state(prev_step, field, response).await
        }
    } else {
        Ok(response.send(GameResponse::NoMoreUndo).await?)
    }
}

#[inline(always)]
async fn send_game_state(
    x: u8,
    y: u8,
    field: &Field,
    response: &Sender<GameResponse>,
) -> Result<()> {
    // send field update
    response
        .send(GameResponse::Field(FieldState { latest: Some((x, y)), field: field.get_field().clone() } ))
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
async fn send_undo_state(prev_step: Option<(u8, u8)>, field: &Field, response: &Sender<GameResponse>) -> Result<()> {
    Ok(response
        .send(GameResponse::Undo(FieldState { latest: prev_step, field: field.get_field().clone() }))
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

#[cfg(test)]
mod test_game {
    use super::*;
    use crate::game::field::Color::{Black, White};
    use async_std::channel::bounded;
    use async_std::task;
    use async_std::task::spawn;
    use futures::executor::block_on;
    use futures::future::join3;

    #[test]
    fn test_do() {
        let (commands, c) = bounded(20);
        let (r, mut response) = bounded(20);
        let game = new_game(0, c, r);
        let sender = async move {
            let test_commands = [
                (5, 5, Black),
                (5, 6, White),
                (6, 6, Black),
                (5, 7, White),
                (4, 4, Black),
                (5, 8, White),
                (7, 7, Black),
                (5, 9, White),
                (8, 8, Black),
            ];
            for (x, y, color) in test_commands {
                commands
                    .send(GameCommand::Do { x, y, color })
                    .await
                    .unwrap();
            }
            for _ in 0..3 {
                commands.send(GameCommand::Undo).await.unwrap();
            }
            commands.send(GameCommand::Kill).await.unwrap();
        };
        let receiver = async {
            while let Some(resp) = response.next().await {
                match resp {
                    GameResponse::Field(f) => {
                        println!("do");
                        println!("{:?}", f);
                    }
                    GameResponse::Undo(f) => {
                        println!("undo");
                        println!("{:?}", f);
                    }
                    _ => println!("{:?}", resp),
                }
            }
        };
        block_on(async { join3(receiver, game, sender).await });
    }
}
