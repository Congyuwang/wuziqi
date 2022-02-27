use crate::game::game_field::Color;
use crate::game::session::messages::{
    PlayerAction, SessionPlayerAction, SessionPlayerResponse, SessionUndoAction, UndoAction,
};
use crate::game::session::utility::TimeoutGate;
use crate::game::session::{
    FieldState, GameQuitResponse, PlayerQuitReason, PlayerResponse, SessionConfig, UndoResponse,
};
use crate::game::Color::Black;
use crate::CHANNEL_SIZE;
use anyhow::Result;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use futures::{stream_select, StreamExt};
#[allow(unused_imports)]
use log::trace;
use std::fmt::{Debug, Formatter};
use std::time::Duration;

pub(crate) fn new_session_player(
    #[allow(unused_variables)] player_id: u64,
    my_color: Color,
    config: SessionConfig,
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
        let mut player_state = PlayerState::new(my_color, responses.clone(), config);
        while let Some(message) = messages.next().await {
            if match message {
                Msg::Player(action) => {
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
                Msg::Session(response) => {
                    #[cfg(debug_assertions)]
                    trace!(
                        "session response \n{:?}\n received by player {}",
                        response,
                        player_id
                    );
                    handle_session_message(
                        my_color,
                        response,
                        &mut player_state,
                        &responses,
                        &killer,
                    )
                    .await
                    .is_err()
                }
                Msg::Kill => {
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
        PlayerAction::Play(x, y) => on_player_play(x, y, player_state).await,
        PlayerAction::RequestUndo => on_request_undo(player_state, responses).await,
        PlayerAction::Undo(undo_action) => on_approving_undo(undo_action, player_state).await,
        PlayerAction::Quit(quit_message) => on_quit_message(quit_message, responses, killer).await,
    }
}

/// handle incoming message from game session
async fn handle_session_message(
    my_color: Color,
    response: SessionPlayerResponse,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
    killer: &Killer,
) -> Result<()> {
    match response {
        SessionPlayerResponse::FieldUpdate(field_state) => {
            on_field_update(my_color, field_state, player_state, responses).await
        }
        SessionPlayerResponse::UndoRequest => {
            on_opponent_undo_request(player_state, responses).await
        }
        SessionPlayerResponse::Undo(undo_rsp) => {
            on_undo_response(my_color, undo_rsp, player_state, responses).await
        }
        SessionPlayerResponse::Quit(quit_rsp) => on_game_quit(quit_rsp, responses, killer).await,
    }
}

/// play when is_my_turn, not_my_turn after play
///
/// ignore out of bound positions
async fn on_player_play(x: u8, y: u8, player_state: &mut PlayerState) -> Result<()> {
    if player_state.undo_dialogue.is_none() && player_state.my_turn.is_some() {
        if x < 15 && y < 15 {
            let timeout_sender = player_state.my_turn.take().unwrap();
            timeout_sender
                .send(Response::Session(SessionPlayerAction::Play(x, y)))
                .await?;
        }
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
async fn on_approving_undo(undo_action: UndoAction, player_state: &mut PlayerState) -> Result<()> {
    if let Some(UndoDialogue::Approving(_)) = &player_state.undo_dialogue {
        debug_assert!(player_state.my_turn.is_some());
        let undo_dialogue = player_state.undo_dialogue.take().unwrap();
        if let UndoDialogue::Approving(timeout_sender) = undo_dialogue {
            match undo_action {
                UndoAction::Approve => {
                    // upon approval, no longer my turn
                    player_state.my_turn = None;
                    timeout_sender
                        .send(Response::Session(SessionPlayerAction::Undo(
                            SessionUndoAction::Approve,
                        )))
                        .await?
                }
                UndoAction::Reject => {
                    timeout_sender
                        .send(Response::Session(SessionPlayerAction::Undo(
                            SessionUndoAction::Reject,
                        )))
                        .await?
                }
            }
            // resume play timer
            player_state.resume_my_turn_timer().await;
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
    my_color: Color,
    field_state: FieldState,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    debug_assert!(player_state.undo_dialogue.is_none());
    if field_state.latest.2 == my_color {
        // when the latest update is my color, allow undo
        player_state.allow_undo = true;
    } else {
        // upon receiving opponent color, ban undo, and is my turn
        player_state.now_my_turn();
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
    if player_state.my_turn.is_some() {
        player_state.approving_undo().await;
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
    my_color: Color,
    undo_rsp: UndoResponse,
    player_state: &mut PlayerState,
    responses: &Sender<Response>,
) -> Result<()> {
    // close undo dialogue once received undo responses from game session
    match &undo_rsp {
        UndoResponse::Undo(f) => {
            match &f.latest {
                // if undid the first step
                None => {
                    if my_color == Black {
                        player_state.now_my_turn();
                    }
                }
                Some((_, _, latest_step_color)) => {
                    // upon receiving undo approval, and it is my turn
                    if *latest_step_color != my_color {
                        player_state.now_my_turn();
                    }
                }
            }
        }
        UndoResponse::TimeoutRejected => {
            // need to resume timer if timeout rejected
            player_state.resume_my_turn_timer().await;
            player_state.undo_dialogue = None;
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

async fn on_game_quit(
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
    debug_assert!(matches!(player_state.undo_dialogue, Some(_)))
}

/// utility for killing player
struct Killer(Sender<Msg>);

impl Killer {
    async fn kill(&self) -> Result<()> {
        Ok(self.0.send(Msg::Kill).await?)
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
) -> (Killer, Receiver<Msg>) {
    let (message_sender, messages) = bounded(CHANNEL_SIZE);
    let killer = message_sender.clone();
    task::spawn(async move {
        let session = session.map(Msg::Session).fuse();
        let player = player.map(Msg::Player).fuse();
        let mut fused = stream_select!(session, player);
        while let Some(message) = fused.next().await {
            if message_sender.send(message).await.is_err() {
                break;
            }
        }
    });
    (Killer(killer), messages)
}

struct PlayerState {
    message_sender: Sender<Response>,
    config: SessionConfig,
    my_turn: Option<TimeoutGate<Response>>,
    allow_undo: bool,
    undo_dialogue: Option<UndoDialogue>,
}

impl PlayerState {
    fn new(my_color: Color, sender: Sender<Response>, config: SessionConfig) -> Self {
        let mut new_state = PlayerState {
            message_sender: sender,
            config,
            my_turn: None,
            allow_undo: false,
            undo_dialogue: None,
        };
        // black first
        if let Color::Black = my_color {
            PlayerState::now_my_turn(&mut new_state)
        }
        new_state
    }

    /// start the timeout immediately, called before calling play
    fn now_my_turn(&mut self) {
        let total_delay = if self.config.play_timeout == 0 {
            None
        } else {
            Some(Duration::from_secs(self.config.play_timeout))
        };
        self.my_turn = Some(TimeoutGate::new(
            total_delay,
            self.message_sender.clone(),
            Response::Session(SessionPlayerAction::PlayTimeout),
        ));
        self.allow_undo = false;
    }

    async fn pause_my_turn_timer(&mut self) {
        if let Some(t_out) = &mut self.my_turn {
            t_out.pause().await
        }
    }

    /// does nothing if it is not in a paused state
    async fn resume_my_turn_timer(&mut self) {
        let extra_time = Duration::from_secs(self.config.undo_dialogue_extra_seconds);
        if let Some(t_out) = &mut self.my_turn {
            t_out.resume(extra_time).await
        }
    }

    /// start the timeout immediately, called before calling play
    async fn approving_undo(&mut self) {
        let total_delay = if self.config.undo_request_timeout == 0 {
            None
        } else {
            Some(Duration::from_secs(self.config.undo_request_timeout))
        };
        self.undo_dialogue = Some(UndoDialogue::Approving(TimeoutGate::new(
            total_delay,
            self.message_sender.clone(),
            Response::Session(SessionPlayerAction::Undo(SessionUndoAction::TimeoutReject)),
        )));
        self.pause_my_turn_timer().await;
    }
}

enum Msg {
    Player(PlayerAction),
    Session(SessionPlayerResponse),
    Kill,
}

enum Response {
    Player(PlayerResponse),
    Session(SessionPlayerAction),
}

enum UndoDialogue {
    Requesting,
    Approving(TimeoutGate<Response>),
}

impl Debug for UndoDialogue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UndoDialogue::Requesting => f.write_str("Requesting"),
            UndoDialogue::Approving(_) => f.write_str("Approving"),
        }
    }
}
