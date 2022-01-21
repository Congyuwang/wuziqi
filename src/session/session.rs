use crate::game::Color::{self, Black, White};
use crate::game::{new_game, GameCommand, GameResponse, State};
use crate::session::messages::GameFinished::BlackWins;
use crate::session::messages::{
    broadcast_to_players, message_receiver, message_sender, GameFinished, PlayerAction,
    PlayerResponse, QuitReason, QuitResponse, SessionKiller, SessionMessage, SessionResponse,
    UndoAction, UndoResponse, CHANNEL_SIZE,
};
use anyhow::Result;
use async_std::channel::{bounded, Receiver, SendError, Sender};
use async_std::task;
use futures::{select, FutureExt, StreamExt};
use log::{error, info, warn};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub async fn new_session(
    game_id: u64,
    black_player: (Sender<PlayerResponse>, Receiver<PlayerAction>, u64),
    white_player: (Sender<PlayerResponse>, Receiver<PlayerAction>, u64),
) {
    // start game thread
    let (cmd_s, rsp_r) = start_game(game_id);
    // start message receiver
    let (killer, mut messages) = message_receiver(black_player.1, white_player.1, rsp_r);
    // start message sender
    let responses = message_sender(black_player.0, white_player.0, cmd_s);
    // Stop this loop by sending `Kill` signal.
    while let Some(message) = messages.next().await {
        if match message {
            // player
            SessionMessage::Player(player_color, player_action) => {
                let player_id = match &player_color {
                    Black => black_player.2,
                    White => white_player.2,
                };
                handle_player_message(player_color, player_action, player_id, &responses, &killer)
                    .await
            }
            SessionMessage::Game(game_rsp) => handle_game_message(game_rsp, &responses).await,
            SessionMessage::Kill(quit_rsp) => {
                log_quit_response(game_id, quit_rsp);
                break;
            }
        }
        .is_err()
        {
            break;
        }
    }
}

/// return Error only when it cannot send
async fn handle_player_message(
    player_color: Color,
    player_action: PlayerAction,
    player_id: u64,
    responses: &Sender<SessionResponse>,
    killer: &SessionKiller,
) -> Result<()> {
    match player_action {
        PlayerAction::Play(x, y) => on_player_play((x, y), player_color, responses).await?,
        PlayerAction::Quit(quit_action) => {
            on_player_quit(quit_action, player_color, player_id, responses, killer).await?
        }
        PlayerAction::RequestUndo => on_player_request_undo(player_color, responses).await?,
        PlayerAction::Undo(undo_action) => {
            on_player_undo(player_color, undo_action, responses).await?
        }
    }
    Ok(())
}

async fn handle_game_message(
    game_message: GameResponse,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    match game_message {
        GameResponse::Field(color, field) => {
            broadcast_to_players(PlayerResponse::FieldUpdate(color, field), responses).await
        }
        GameResponse::Undo(field) => {
            broadcast_to_players(PlayerResponse::Undo(UndoResponse::Undo(field)), responses).await
        }
        GameResponse::BlackWins => {
            broadcast_to_players(
                PlayerResponse::Quit(QuitResponse::GameEnd(GameFinished::BlackWins)),
                responses,
            )
            .await
        }
        GameResponse::WhiteWins => {
            broadcast_to_players(
                PlayerResponse::Quit(QuitResponse::GameEnd(GameFinished::WhiteWins)),
                responses,
            )
            .await
        }
        GameResponse::Draw => {
            broadcast_to_players(
                PlayerResponse::Quit(QuitResponse::GameEnd(GameFinished::Draw)),
                responses,
            )
            .await
        }
        GameResponse::NoMoreUndo => {
            broadcast_to_players(PlayerResponse::Undo(UndoResponse::NoMoreUndo), responses).await
        }
        GameResponse::GameError(e) => {
            broadcast_to_players(PlayerResponse::Quit(QuitResponse::GameError(e)), responses).await
        }
    }
}

async fn on_player_request_undo(
    player_color: Color,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    Ok(responses
        .send(SessionResponse::Player(
            player_color.switch(),
            PlayerResponse::UndoRequest,
        ))
        .await?)
}

/// handle events when player plays a step
async fn on_player_play(
    (x, y): (u8, u8),
    color: Color,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    Ok(responses
        .send(SessionResponse::Game(GameCommand::Do { x, y, color }))
        .await?)
}

/// handle events when player plays a step
async fn on_player_undo(
    color: Color,
    undo_action: UndoAction,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    match undo_action {
        UndoAction::Approve => {
            responses
                .send(SessionResponse::Game(GameCommand::Undo))
                .await?
        }
        UndoAction::Reject => {
            responses
                .send(SessionResponse::Player(
                    color.switch(),
                    PlayerResponse::Undo(UndoResponse::RejectedByOpponent),
                ))
                .await?
        }
        UndoAction::AutoReject => {
            responses
                .send(SessionResponse::Player(
                    color.switch(),
                    PlayerResponse::Undo(UndoResponse::AutoRejected),
                ))
                .await?
        }
        UndoAction::TimeOutReject => {
            responses
                .send(SessionResponse::Player(
                    color.switch(),
                    PlayerResponse::Undo(UndoResponse::TimeOutRejected),
                ))
                .await?
        }
    }
    Ok(())
}

/// handle event when one player quits game
async fn on_player_quit(
    quit_action: QuitReason,
    player_color: Color,
    player_id: u64,
    responses: &Sender<SessionResponse>,
    killer: &SessionKiller,
) -> Result<()> {
    // the reason for player's quit action
    let quit_rsp = match quit_action {
        QuitReason::PlayerQuit => QuitResponse::PlayerQuit(player_id),
        QuitReason::PlayerDisconnected => QuitResponse::PlayerDisconnected(player_id),
        QuitReason::PlayerError(e) => QuitResponse::PlayerError(player_id, e),
    };
    // notify the other player
    responses
        .send(SessionResponse::Player(
            player_color.switch(),
            PlayerResponse::Quit(quit_rsp.clone()),
        ))
        .await?;
    // kill game
    responses
        .send(SessionResponse::Game(GameCommand::Kill))
        .await?;
    // kill game session
    killer.kill(quit_rsp).await
}

fn log_quit_response(game_id: u64, quit_rsp: QuitResponse) {
    match quit_rsp {
        QuitResponse::GameEnd(e) => {
            error!("game {} finished in {}", game_id, e)
        }
        QuitResponse::PlayerQuit(player_id) => {
            info!("player {} quit game {}", player_id, game_id)
        }
        QuitResponse::PlayerDisconnected(player_id) => {
            warn!("player {} disconnected from game {}", player_id, game_id)
        }
        QuitResponse::PlayerError(player_id, e) => error!(
            "player {} error in game {}. Error: {}",
            player_id, game_id, e
        ),
        QuitResponse::GameError(e) => error!("game {} got error {}", game_id, e),
    }
}

/// stop game by sending `Kill` or by dropping `Receiver<GameResponse>` and `Sender<GameCommand>`
fn start_game(game_id: u64) -> (Sender<GameCommand>, Receiver<GameResponse>) {
    let (cmd_s, cmd_r) = bounded(CHANNEL_SIZE);
    let (rsp_s, rsp_r) = bounded(CHANNEL_SIZE);
    task::spawn(new_game(game_id, cmd_r, rsp_s));
    (cmd_s, rsp_r)
}
