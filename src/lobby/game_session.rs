use crate::game::Color::{Black, White};
use crate::game::{
    new_session, Color, Commands, GameQuitResponse, GameResult, PlayerQuitReason, PlayerResponse,
    SessionConfig, UndoResponse,
};
use crate::lobby::client_connection::ClientConnection;
use crate::lobby::messages::{Messages, Responses};
use crate::stream_utility::Plug;
use crate::CHANNEL_SIZE;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use async_std::task::JoinHandle;
use futures::{select, StreamExt};
use std::fmt::{Display, Formatter};

pub(crate) enum ExitState {
    ReturnRoom(ClientConnection, PlayerResult),
    ExitGame,
}

pub(crate) enum PlayerResult {
    Win,
    Lose,
    Draw,
    Quit,
    OpponentQuit,
}

pub(crate) async fn start_game_session(
    session_id: u64,
    black_player_id: u64,
    white_player_id: u64,
    session_config: SessionConfig,
    black_player: ClientConnection,
    white_player: ClientConnection,
) -> (ExitState, ExitState) {
    let (black_cmd, white_cmd) =
        new_session(session_id, black_player_id, white_player_id, session_config);
    let (b_chat_s, b_chat_r) = bounded(CHANNEL_SIZE);
    let (w_chat_s, w_chat_r) = bounded(CHANNEL_SIZE);
    // send start messages
    let _ = black_player
        .sender()
        .send(Responses::GameStarted(Black))
        .await;
    let _ = white_player
        .sender()
        .send(Responses::GameStarted(White))
        .await;
    let b_exit = connect_player_game(
        black_player_id,
        black_player,
        black_cmd,
        b_chat_r,
        &w_chat_s,
        Black,
    );
    let w_exit = connect_player_game(
        white_player_id,
        white_player,
        white_cmd,
        w_chat_r,
        &b_chat_s,
        White,
    );
    (b_exit.await, w_exit.await)
}

/// this function connects a `ClientConnection` with `Commands`.
fn connect_player_game(
    player_id: u64,
    player: ClientConnection,
    mut command: Commands,
    chat_receiver: Receiver<(String, String)>,
    chat_sender: &Sender<(String, String)>,
    color: Color,
) -> JoinHandle<ExitState> {
    let session_rsp = command.get_listener().unwrap();
    let player_sender = player.sender().clone();
    let player_chat_sender = player.sender().clone();
    let player_name = player.player_name().to_string();
    let chat_sender = chat_sender.clone();
    let (mut chat_receiver, chat_stopper) = Plug::new(chat_receiver);
    // send chat messages
    task::spawn(async move {
        while let Some((name, msg)) = chat_receiver.next().await {
            let _ = player_chat_sender
                .send(Responses::ChatMessage(name, msg))
                .await;
        }
    });
    // handle session response and player commands
    task::spawn(async move {
        let mut player = player.fuse();
        let mut session = session_rsp.fuse();
        loop {
            let next_step = select! {
                cmd = player.next() => {
                    handle_command(cmd, &command, &player_name, &chat_sender).await
                }
                rsp = session.next() => {
                    handle_session_response(player_id, rsp, &player_sender, color).await
                }
            } as NextStep;
            match next_step {
                NextStep::EnterLobby(result) => {
                    let _ = chat_stopper.unplug().await;
                    break ExitState::ReturnRoom(player.into_inner(), result);
                }
                NextStep::ExitGame => {
                    let _ = chat_stopper.unplug().await;
                    break ExitState::ExitGame;
                }
                NextStep::Continue => {}
            }
        }
    })
}

enum NextStep {
    EnterLobby(PlayerResult),
    ExitGame,
    Continue,
}

async fn handle_command(
    msg: Option<Messages>,
    command: &Commands,
    player_name: &str,
    chat_sender: &Sender<(String, String)>,
) -> NextStep {
    if let Some(msg) = msg {
        match msg {
            Messages::Play(x, y) => command.play(x, y).await,
            Messages::RequestUndo => command.request_undo().await,
            Messages::ApproveUndo => command.approve_undo().await,
            Messages::RejectUndo => command.reject_undo().await,
            Messages::ChatMessage(msg) => {
                let _ = chat_sender.send((player_name.to_string(), msg)).await;
            }
            Messages::QuitGameSession => {
                command.quit(PlayerQuitReason::QuitSession).await;
                return NextStep::EnterLobby(PlayerResult::Quit);
            }
            Messages::ExitGame => {
                command.quit(PlayerQuitReason::ExitGame).await;
                return NextStep::ExitGame;
            }
            Messages::ClientError(e) => {
                command.quit(PlayerQuitReason::Error(e)).await;
                return NextStep::ExitGame;
            }
            _ => {}
        };
        NextStep::Continue
    } else {
        // player disconnected (maybe due to connection error)
        command.quit(PlayerQuitReason::Disconnected).await;
        NextStep::ExitGame
    }
}

// do not need to handle disconnection at sending message
async fn handle_session_response(
    my_id: u64,
    rsp: Option<PlayerResponse>,
    player_sender: &Sender<Responses>,
    color: Color,
) -> NextStep {
    match rsp {
        Some(rsp) => {
            let _ = match rsp {
                PlayerResponse::FieldUpdate(f) => {
                    player_sender.send(Responses::FieldUpdate(f)).await
                }
                PlayerResponse::UndoRequest => player_sender.send(Responses::UndoRequest).await,
                PlayerResponse::Undo(u_rsp) => match u_rsp {
                    UndoResponse::TimeoutRejected => {
                        player_sender.send(Responses::UndoTimeoutRejected).await
                    }
                    UndoResponse::Undo(f) => player_sender.send(Responses::Undo(f)).await,
                    UndoResponse::RejectedByOpponent => {
                        player_sender.send(Responses::UndoRejectedByOpponent).await
                    }
                    UndoResponse::AutoRejected => {
                        player_sender.send(Responses::UndoAutoRejected).await
                    }
                },
                PlayerResponse::Quit(q) => {
                    return match q {
                        GameQuitResponse::GameEnd(end) => match end {
                            GameResult::BlackTimeout => {
                                let _ = player_sender.send(Responses::GameEndBlackTimeout).await;
                                match color {
                                    Black => NextStep::EnterLobby(PlayerResult::Lose),
                                    White => NextStep::EnterLobby(PlayerResult::Win),
                                }
                            }
                            GameResult::WhiteTimeout => {
                                let _ = player_sender.send(Responses::GameEndWhiteTimeout).await;
                                match color {
                                    Black => NextStep::EnterLobby(PlayerResult::Win),
                                    White => NextStep::EnterLobby(PlayerResult::Lose),
                                }
                            }
                            GameResult::BlackWins => {
                                let _ = player_sender.send(Responses::GameEndBlackWins).await;
                                match color {
                                    Black => NextStep::EnterLobby(PlayerResult::Win),
                                    White => NextStep::EnterLobby(PlayerResult::Lose),
                                }
                            }
                            GameResult::WhiteWins => {
                                let _ = player_sender.send(Responses::GameEndWhiteWins).await;
                                match color {
                                    Black => NextStep::EnterLobby(PlayerResult::Lose),
                                    White => NextStep::EnterLobby(PlayerResult::Win),
                                }
                            }
                            GameResult::Draw => {
                                let _ = player_sender.send(Responses::GameEndDraw).await;
                                NextStep::EnterLobby(PlayerResult::Draw)
                            }
                        },
                        GameQuitResponse::PlayerQuitSession(id) => {
                            if id == my_id {
                                let _ = player_sender.send(Responses::QuitGameSessionSuccess).await;
                            } else {
                                let _ =
                                    player_sender.send(Responses::OpponentQuitGameSession).await;
                            }
                            // enter lobby on opponent quit
                            NextStep::EnterLobby(PlayerResult::OpponentQuit)
                        }
                        GameQuitResponse::OpponentExitGame(_) => {
                            let _ = player_sender.send(Responses::OpponentExitGame).await;
                            NextStep::EnterLobby(PlayerResult::OpponentQuit)
                        }
                        GameQuitResponse::OpponentDisconnected(_) => {
                            let _ = player_sender.send(Responses::OpponentDisconnected).await;
                            NextStep::EnterLobby(PlayerResult::OpponentQuit)
                        }
                        GameQuitResponse::OpponentError(_, e) => {
                            let _ = player_sender
                                .send(Responses::GameSessionError(format!("player error: {}", e)))
                                .await;
                            NextStep::EnterLobby(PlayerResult::OpponentQuit)
                        }
                        GameQuitResponse::GameError(e) => {
                            let _ = player_sender
                                .send(Responses::GameSessionError(format!("game error: {}", e)))
                                .await;
                            NextStep::ExitGame
                        }
                    };
                }
            };
            NextStep::Continue
        }
        None => unreachable!(),
    }
}

impl Display for PlayerResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PlayerResult::Win => "win",
            PlayerResult::Lose => "lose",
            PlayerResult::Draw => "draw",
            PlayerResult::Quit => "quit",
            PlayerResult::OpponentQuit => "opponent_quit",
        })
    }
}
