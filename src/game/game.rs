use crate::game::field::{Color, Field, FieldState, State};
use anyhow::{Error, Result};
use async_std::channel::{Receiver, SendError, Sender};
use futures::StreamExt;
use log::{error, info};
use std::collections::VecDeque;

#[derive(Debug)]
pub enum GameCommand {
    Do { x: u8, y: u8, color: Color },
    Undo,
    Kill,
}

#[derive(Debug)]
pub enum GameResponse {
    Field([[State; 15]; 15]),
    BlackWins,
    WhiteWins,
    Draw,
    NoMoreUndo,
    GameError(String),
    GameStopped,
}

/// start a new game, this is the only public API
pub async fn new_game(
    game_id: usize,
    mut commands: Receiver<GameCommand>,
    mut response: Sender<GameResponse>,
) {
    let mut history = VecDeque::with_capacity(225);
    let mut field = Field::new();
    while let Some(command) = commands.next().await {
        println!("{:?}", command);
        if let Err(e) =
            execute_command(game_id, &mut field, &command, &response, &mut history).await
        {
            info!("game no {} stopped: {}", game_id, e);
            // ignore this potential error
            let _ = response.send(GameResponse::GameStopped).await;
            break;
        }
    }
}

/// the error of this function means game killed or receivers dropped, just exit
async fn execute_command(
    game_id: usize,
    field: &mut Field,
    command: &GameCommand,
    response: &Sender<GameResponse>,
    history: &mut VecDeque<(u8, u8, Color)>,
) -> Result<()> {
    match &command {
        GameCommand::Do { x, y, color } => {
            do_play(game_id, field, *x, *y, color, history, response).await
        }
        GameCommand::Undo => undo_play(game_id, field, history, response).await,
        GameCommand::Kill => Err(Error::msg("game killed")),
    }
}

async fn do_play(
    game_id: usize,
    field: &mut Field,
    x: u8,
    y: u8,
    color: &Color,
    history: &mut VecDeque<(u8, u8, Color)>,
    response: &Sender<GameResponse>,
) -> Result<()> {
    if let Err(e) = field.play(x as usize, y as usize, *color) {
        send_unlikely_error(e, game_id, response).await?;
    } else {
        history.push_back((x, y, *color));
        send_game_state(field, response).await?;
    }
    Ok(())
}

/// the error of this function can only come from being receivers being closed, just exit
async fn undo_play(
    game_id: usize,
    field: &mut Field,
    history: &mut VecDeque<(u8, u8, Color)>,
    response: &Sender<GameResponse>,
) -> Result<()> {
    if let Some((x, y, _)) = history.pop_back() {
        if let Err(e) = field.clear(x as usize, y as usize) {
            send_unlikely_error(e, game_id, response).await?;
        } else {
            send_game_state(field, response).await?;
        }
    } else {
        response.send(GameResponse::NoMoreUndo).await?;
    }
    Ok(())
}

#[inline(always)]
async fn send_game_state(field: &Field, response: &Sender<GameResponse>) -> Result<()> {
    // send field update
    response
        .send(GameResponse::Field(field.get_field().clone()))
        .await?;
    // send field state
    Ok(match field.get_field_state() {
        FieldState::BlackWins => response.send(GameResponse::BlackWins).await?,
        FieldState::WhiteWins => response.send(GameResponse::WhiteWins).await?,
        FieldState::Draw => response.send(GameResponse::Draw).await?,
        FieldState::Impossible => response.send(GameResponse::GameError("impossible_game_state".to_string())).await?,
        FieldState::UnFinished => (),
    })
}

/// the error of this function can only come from being receivers being closed, just exit
#[cold]
async fn send_unlikely_error(e: Error, game_id: usize, response: &Sender<GameResponse>) -> Result<()> {
    error!("game no {} error: {}", game_id, e);
    Ok(response.send(GameResponse::GameError(e.to_string())).await?)
}

#[cfg(test)]
mod test_game {
    use async_std::channel::bounded;
    use async_std::task;
    use async_std::task::spawn;
    use futures::executor::block_on;
    use futures::future::{join,join3};
    use crate::game::field::Color::{Black, White};
    use super::*;

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
                commands.send(GameCommand::Do { x, y, color }).await.unwrap();
            }
            for _ in 0..3 {
                commands.send(GameCommand::Undo).await.unwrap();
            }
            commands.send(GameCommand::Kill).await.unwrap();
        };
        let receiver = async {
            while let Some(resp) = response.next().await {
                if let GameResponse::Field(f) = resp {
                    println!("{:?}", f);
                } else {
                    println!("{:?}", resp);
                }
            }
        };
        block_on(async { join3(game, sender, receiver).await });
    }
}
