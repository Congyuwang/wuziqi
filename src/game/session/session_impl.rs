use crate::game::game_field::Color::{self, Black, White};
use crate::game::game_field::{new_field, GameCommand, GameResponse};
use crate::game::session::api::SessionConfig;
use crate::game::session::api::{
    Commands, GameQuitResponse, GameResult, PlayerQuitReason, UndoResponse,
};
use crate::game::session::messages::{
    broadcast_to_players, message_receiver, message_sender, SessionKiller, SessionMessage,
    SessionPlayerAction, SessionPlayerResponse, SessionResponse, SessionUndoAction,
};
use crate::game::session::player::new_session_player;
use anyhow::Result;
use async_std::channel::Sender;
use async_std::task;
use futures::StreamExt;
#[allow(unused_imports)]
use log::trace;
use log::{error, info, warn};

/// start a new game session
pub fn new_session(
    session_id: u64,
    black_player_id: u64,
    white_player_id: u64,
    session_config: SessionConfig,
) -> (Commands, Commands) {
    info!(
        "game session {} launched with black player {} and white player {}",
        session_id, black_player_id, white_player_id
    );
    // start player tasks
    let black_player = new_session_player(black_player_id, Black, session_config.clone());
    let white_player = new_session_player(white_player_id, White, session_config);
    // start field task
    let (cmd, rsp) = new_field(session_id);
    // start message receiver task
    let (killer, mut messages) = message_receiver(black_player.2, white_player.2, rsp);
    // start message sender task
    let responses = message_sender(black_player.3, white_player.3, cmd);
    task::spawn(async move {
        while let Some(message) = messages.next().await {
            #[cfg(debug_assertions)]
            trace!("message {:?} received by session {}", message, session_id);
            if match message {
                // player
                SessionMessage::Player(player_color, player_action) => {
                    let player_id = match &player_color {
                        Black => black_player_id,
                        White => white_player_id,
                    };
                    handle_player_message(
                        player_color,
                        player_action,
                        player_id,
                        &responses,
                        &killer,
                    )
                    .await
                }
                SessionMessage::Game(game_rsp) => handle_game_message(game_rsp, &responses).await,
                SessionMessage::Kill(quit_rsp) => {
                    log_quit_response(session_id, quit_rsp);
                    break;
                }
            }
            .is_err()
            {
                #[cfg(debug_assertions)]
                trace!(
                    "session thread of game {} and player {} and {} stopped",
                    session_id,
                    black_player_id,
                    white_player_id
                );
                break;
            }
        }
        #[cfg(debug_assertions)]
        trace!(
            "session thread of game {} and player {} and {} stopped",
            session_id,
            black_player_id,
            white_player_id
        )
    });
    (
        Commands::new(black_player.0, black_player.1),
        Commands::new(white_player.0, white_player.1),
    )
}

/// return Error only when it cannot send
async fn handle_player_message(
    player_color: Color,
    player_action: SessionPlayerAction,
    player_id: u64,
    responses: &Sender<SessionResponse>,
    killer: &SessionKiller,
) -> Result<()> {
    match player_action {
        SessionPlayerAction::Play(x, y) => on_player_play((x, y), player_color, responses).await?,
        SessionPlayerAction::Quit(quit_action) => {
            on_player_quit(quit_action, player_color, player_id, responses, killer).await?
        }
        SessionPlayerAction::RequestUndo => on_player_request_undo(player_color, responses).await?,
        SessionPlayerAction::Undo(undo_action) => {
            on_player_undo(player_color, undo_action, responses).await?
        }
        SessionPlayerAction::PlayTimeout => on_player_timeout(player_color, responses).await?,
    }
    Ok(())
}

async fn handle_game_message(
    game_message: GameResponse,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    match game_message {
        GameResponse::Field(state) => {
            broadcast_to_players(SessionPlayerResponse::FieldUpdate(state), responses).await
        }
        GameResponse::Undo(field) => {
            broadcast_to_players(
                SessionPlayerResponse::Undo(UndoResponse::Undo(field)),
                responses,
            )
            .await
        }
        GameResponse::BlackWins => {
            broadcast_to_players(
                SessionPlayerResponse::Quit(GameQuitResponse::GameEnd(GameResult::BlackWins)),
                responses,
            )
            .await
        }
        GameResponse::WhiteWins => {
            broadcast_to_players(
                SessionPlayerResponse::Quit(GameQuitResponse::GameEnd(GameResult::WhiteWins)),
                responses,
            )
            .await
        }
        GameResponse::Draw => {
            broadcast_to_players(
                SessionPlayerResponse::Quit(GameQuitResponse::GameEnd(GameResult::Draw)),
                responses,
            )
            .await
        }
        GameResponse::GameError(e) => {
            broadcast_to_players(
                SessionPlayerResponse::Quit(GameQuitResponse::GameError(e)),
                responses,
            )
            .await
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
            SessionPlayerResponse::UndoRequest,
        ))
        .await?)
}

async fn on_player_timeout(player_color: Color, responses: &Sender<SessionResponse>) -> Result<()> {
    let quit_rsp = match player_color {
        Black => GameQuitResponse::GameEnd(GameResult::BlackTimeout),
        White => GameQuitResponse::GameEnd(GameResult::WhiteTimeout),
    };
    broadcast_to_players(SessionPlayerResponse::Quit(quit_rsp), responses).await?;
    Ok(())
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
    undo_action: SessionUndoAction,
    responses: &Sender<SessionResponse>,
) -> Result<()> {
    match undo_action {
        SessionUndoAction::Approve => {
            responses
                .send(SessionResponse::Game(GameCommand::Undo))
                .await?
        }
        SessionUndoAction::Reject => {
            responses
                .send(SessionResponse::Player(
                    color.switch(),
                    SessionPlayerResponse::Undo(UndoResponse::RejectedByOpponent),
                ))
                .await?
        }
        SessionUndoAction::AutoReject => {
            responses
                .send(SessionResponse::Player(
                    color.switch(),
                    SessionPlayerResponse::Undo(UndoResponse::AutoRejected),
                ))
                .await?
        }
        SessionUndoAction::TimeoutReject => {
            broadcast_to_players(
                SessionPlayerResponse::Undo(UndoResponse::TimeoutRejected),
                responses,
            )
            .await?;
        }
    }
    Ok(())
}

/// handle event when one player quits game
async fn on_player_quit(
    quit_action: PlayerQuitReason,
    player_color: Color,
    player_id: u64,
    responses: &Sender<SessionResponse>,
    killer: &SessionKiller,
) -> Result<()> {
    // the reason for player's quit action
    let quit_rsp = match quit_action {
        PlayerQuitReason::QuitSession => GameQuitResponse::OpponentQuitSession(player_id),
        PlayerQuitReason::Disconnected => GameQuitResponse::OpponentDisconnected(player_id),
        PlayerQuitReason::Error(e) => GameQuitResponse::OpponentError(player_id, e),
        PlayerQuitReason::ExitGame => GameQuitResponse::OpponentExitGame(player_id),
    };
    // notify the other player
    responses
        .send(SessionResponse::Player(
            player_color.switch(),
            SessionPlayerResponse::Quit(quit_rsp.clone()),
        ))
        .await?;
    // kill game
    responses
        .send(SessionResponse::Game(GameCommand::Kill))
        .await?;
    // kill game session
    killer.kill(quit_rsp).await
}

fn log_quit_response(game_id: u64, quit_rsp: GameQuitResponse) {
    match quit_rsp {
        GameQuitResponse::GameEnd(e) => {
            error!("game {} finished in error {}", game_id, e)
        }
        GameQuitResponse::OpponentQuitSession(player_id) => {
            info!("player {} quit game {}", player_id, game_id)
        }
        GameQuitResponse::OpponentExitGame(player_id) => {
            info!("player {} exit game {}", player_id, game_id)
        }
        GameQuitResponse::OpponentDisconnected(player_id) => {
            warn!("player {} disconnected from game {}", player_id, game_id)
        }
        GameQuitResponse::OpponentError(player_id, e) => error!(
            "player {} error in game {}. Error: {}",
            player_id, game_id, e
        ),
        GameQuitResponse::GameError(e) => error!("game {} got error {}", game_id, e),
    }
}
