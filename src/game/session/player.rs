use std::time::Duration;
use crate::game::game_field::Color;
use crate::game::session::messages::{
    PlayerAction, SessionPlayerAction, SessionPlayerResponse, SessionUndoAction, UndoAction,
};
use crate::game::session::{
    FieldState, GameQuitResponse, PlayerQuitReason, PlayerResponse, UndoResponse,
};
use crate::CHANNEL_SIZE;
use anyhow::Result;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use futures::{stream_select, StreamExt};
use log::trace;

pub(crate) fn new_session_player(
    player_id: u64,
    color: Color,
) -> (
    Sender<PlayerAction>,
    Receiver<PlayerResponse>,
    Receiver<SessionPlayerAction>,
    Sender<SessionPlayerResponse>,
) {
    let action_pipe_to_session = bounded(CHANNEL_SIZE);
    let response_pipe_to_session = bounded(CHANNEL_SIZE);
    let pub_action_pipe = bounded(CHANNEL_SIZE);
    let pub_response_pipe = bounded(CHANNEL_SIZE);

    task::spawn(async move {
        let responses = message_sender(action_pipe_to_session.0, pub_response_pipe.0);
        let (killer, mut messages) =
            message_receiver(response_pipe_to_session.1, pub_action_pipe.1);
        let mut player_state = PlayerState::new(&color);
        while let Some(message) = messages.next().await {
            if match message {
                Message::Player(action) => {
                    #[cfg(debug_assertions)]
                    trace!(
                        "remote player action \n{:?}\n received by player {}",
                        action,
                        player_id
                    );
                    handle_player_message(action, &mut player_state, &responses, &killer)
                        .await
                        .is_err()
                }
                Message::Session(response) => {
                    #[cfg(debug_assertions)]
                    trace!(
                        "session response \n{:?}\n received by player {}",
                        response,
                        player_id
                    );
                    handle_session_message(color, response, &mut player_state, &responses, &killer)
                        .await
                        .is_err()
                }
                Message::Kill => {
                    #[cfg(debug_assertions)]
                    trace!("player {} killed", player_id);
                    break;
                }
            } {
                #[cfg(debug_assertions)]
                trace!("player {} stopped on err", player_id);
                break;
            }
        }
    });

    (
        pub_action_pipe.0,
        pub_response_pipe.1,
        action_pipe_to_session.1,
        response_pipe_to_session.0,
    )
}

/// handle incoming message from player client
async fn handle_player_message(
    action: PlayerAction,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
    killer: &Killer,
) -> Result<()> {
    match action {
        PlayerAction::Play(x, y) => on_player_play(x, y, player_state, responses).await,
        PlayerAction::RequestUndo => on_request_undo(player_state, responses).await,
        PlayerAction::Undo(undo_action) => {
            on_approving_undo(undo_action, player_state, responses).await
        }
        PlayerAction::Quit(quit_message) => on_quit_message(quit_message, responses, killer).await,
    }
}

/// handle incoming message from game session
async fn handle_session_message(
    color: Color,
    response: SessionPlayerResponse,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
    killer: &Killer,
) -> Result<()> {
    match response {
        SessionPlayerResponse::FieldUpdate(field_state) => {
            on_field_update(color, field_state, player_state, responses).await
        }
        SessionPlayerResponse::UndoRequest => {
            on_opponent_undo_request(player_state, responses).await
        }
        SessionPlayerResponse::Undo(undo_rsp) => {
            on_undo_response(color, undo_rsp, player_state, responses).await
        }
        SessionPlayerResponse::Quit(quit_rsp) => {
            on_opponent_quit(quit_rsp, responses, killer).await
        }
    }
}

/// play when is_my_turn, not_my_turn after play
async fn on_player_play(
    x: u8,
    y: u8,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    if player_state.undo_dialogue.is_none() && player_state.is_my_turn {
        player_state.is_my_turn = false;
        responses
            .send(Response::Session(SessionPlayerAction::Play(x, y)))
            .await?
    }
    Ok(())
}

/// send undo request when allow undo, ban undo after sending the request
async fn on_request_undo(
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    // undo when allow_undo
    if player_state.undo_dialogue.is_none() && player_state.allow_undo {
        player_state.allow_undo = false;
        player_state.undo_dialogue = Some(UndoDialogue::Requesting);
        responses
            .send(Response::Session(SessionPlayerAction::RequestUndo))
            .await?
    }
    Ok(())
}

/// send undo approval or rejection when in approving dialogue
async fn on_approving_undo(
    undo_action: UndoAction,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    if let Some(UndoDialogue::Approving) = player_state.undo_dialogue {
        match undo_action {
            UndoAction::Approve => {
                player_state.is_my_turn = false;
                responses
                    .send(Response::Session(SessionPlayerAction::Undo(
                        SessionUndoAction::Approve,
                    )))
                    .await?
            }
            UndoAction::Reject => {
                player_state.undo_dialogue = None;
                responses
                    .send(Response::Session(SessionPlayerAction::Undo(
                        SessionUndoAction::Reject,
                    )))
                    .await?
            }
        }
    }
    Ok(())
}

/// quit, disconnect, error
async fn on_quit_message(
    quit_message: PlayerQuitReason,
    responses: &Sender<Response>,
    killer: &Killer,
) -> Result<()> {
    // send quit message to session and stop
    responses
        .send(Response::Session(SessionPlayerAction::Quit(quit_message)))
        .await?;
    killer.kill().await?;
    Ok(())
}

/// receiving play responses from either me or opponent
async fn on_field_update(
    color: Color,
    field_state: FieldState,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    debug_assert!(player_state.undo_dialogue.is_none());
    if field_state.latest.2 == color {
        // when the latest update is my color, allow undo
        player_state.allow_undo = true;
    } else {
        // upon receiving opponent color, ban undo, and is my turn
        player_state.is_my_turn = true;
        player_state.allow_undo = false;
    }
    // forward field state
    responses
        .send(Response::Player(PlayerResponse::FieldUpdate(field_state)))
        .await?;
    Ok(())
}

/// on receiving undo request from opponent, forward undo_request to client
async fn on_opponent_undo_request(
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    debug_assert!(player_state.undo_dialogue.is_none());
    if player_state.is_my_turn {
        player_state.undo_dialogue = Some(UndoDialogue::Approving);
        responses
            .send(Response::Player(PlayerResponse::UndoRequest))
            .await?;
    } else {
        // auto reject undo request when I have already moved
        responses
            .send(Response::Session(SessionPlayerAction::Undo(
                SessionUndoAction::AutoReject,
            )))
            .await?
    }
    Ok(())
}

/// when player receives undo response from game session
async fn on_undo_response(
    color: Color,
    undo_rsp: UndoResponse,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    // close undo dialogue once received undo responses from game session
    debug_assert!(player_state.undo_dialogue.is_some());
    match &undo_rsp {
        UndoResponse::Undo(f) => {
            match &f.latest {
                // if undid the first step
                None => {
                    player_state.is_my_turn = match color {
                        Color::Black => true,
                        Color::White => false,
                    };
                    player_state.allow_undo = false;
                }
                Some((_, _, c)) => {
                    // upon receiving undo approval, and it is mu turn
                    if *c != color {
                        player_state.is_my_turn = true;
                    }
                }
            }
        }
        _ => {
            #[cfg(debug_assertions)]
            assert_is_requesting_undo(player_state);
        }
    }
    // forward undo response, close undo dialogue
    player_state.undo_dialogue = None;
    responses
        .send(Response::Player(PlayerResponse::Undo(undo_rsp)))
        .await?;
    Ok(())
}

async fn on_opponent_quit(
    quit_rsp: GameQuitResponse,
    responses: &Sender<Response>,
    killer: &Killer,
) -> Result<()> {
    responses
        .send(Response::Player(PlayerResponse::Quit(quit_rsp)))
        .await?;
    killer.kill().await?;
    Ok(())
}

/// state debug assertions, not compiled in release profile
#[cfg(debug_assertions)]
fn assert_is_requesting_undo(player_state: &PlayerState) {
    debug_assert!(matches!(
        player_state.undo_dialogue,
        Some(UndoDialogue::Requesting)
    ))
}

/// utility for killing player
struct Killer(Sender<Message>);

impl Killer {
    async fn kill(&self) -> Result<()> {
        Ok(self.0.send(Message::Kill).await?)
    }
}

/// utility: distribute message
fn message_sender(
    session: Sender<SessionPlayerAction>,
    player: Sender<PlayerResponse>,
) -> Sender<Response> {
    let (sender, mut receiver) = bounded(CHANNEL_SIZE);
    task::spawn(async move {
        while let Some(session_response) = receiver.next().await {
            if match session_response {
                Response::Player(rsp) => player.send(rsp).await.is_err(),
                Response::Session(act) => session.send(act).await.is_err(),
            } {
                break;
            }
        }
    });
    sender
}

/// utility: fuse messages
fn message_receiver(
    session: Receiver<SessionPlayerResponse>,
    player: Receiver<PlayerAction>,
) -> (Killer, Receiver<Message>) {
    let (message_sender, messages) = bounded(CHANNEL_SIZE);
    let killer = message_sender.clone();
    task::spawn(async move {
        let session = session.map(Message::Session).fuse();
        let player = player.map(Message::Player).fuse();
        let mut fused = stream_select!(session, player);
        while let Some(message) = fused.next().await {
            if message_sender.send(message).await.is_err() {
                break;
            }
        }
    });
    (Killer(killer), messages)
}

impl PlayerState {
    fn new(color: &Color) -> Self {
        match color {
            Color::Black => PlayerState {
                is_my_turn: true,
                allow_undo: false,
                undo_dialogue: None,
            },
            Color::White => PlayerState {
                is_my_turn: false,
                allow_undo: false,
                undo_dialogue: None,
            },
        }
    }
}

struct PlayerState {
    is_my_turn: bool,
    allow_undo: bool,
    undo_dialogue: Option<UndoDialogue>,
}

enum Message {
    Player(PlayerAction),
    Session(SessionPlayerResponse),
    Kill,
}

enum Response {
    Player(PlayerResponse),
    Session(SessionPlayerAction),
}

#[derive(Debug)]
enum UndoDialogue {
    Requesting,
    Approving,
}
