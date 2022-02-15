use crate::game::{
    new_session, Commands, GameQuitResponse, GameResult, PlayerQuitReason, PlayerResponse,
    SessionConfig, UndoResponse,
};
use crate::lobby::messages::{ClientConnection, Messages, Responses};
use crate::lobby::token::RoomToken;
use crate::network_util::{ConnectionError, Received};
use crate::CHANNEL_SIZE;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use async_std::task::JoinHandle;
use futures::{select, StreamExt};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use futures::future::join;
use crate::stream_utility::{Next, Pause};

fn start_game_session(
    session_id: u64,
    black_player_id: u64,
    white_player_id: u64,
    session_config: SessionConfig,
    black_player: ClientConnection,
    white_player: ClientConnection,
) -> (JoinHandle<ExitState>, JoinHandle<ExitState>)
{
    let (black_cmd, white_cmd) =
        new_session(session_id, black_player_id, white_player_id, session_config);
    let (b_chat_s, b_chat_r) = bounded(CHANNEL_SIZE);
    let (w_chat_s, w_chat_r) = bounded(CHANNEL_SIZE);
    let b_exit = connect_player_game(black_player, black_cmd, b_chat_r, &w_chat_s);
    let w_exit = connect_player_game(white_player, white_cmd, w_chat_r, &b_chat_s);
    (b_exit, w_exit)
}

enum ExitState {
    EnterLobby(ClientConnection),
    ExitGame,
}

fn connect_player_game(
    mut player: ClientConnection,
    mut command: Commands,
    chat_receiver: Receiver<String>,
    chat_sender: &Sender<String>,
) -> JoinHandle<ExitState> {
    let session_rsp = command.get_listener().unwrap();
    let player_sender = player.sender().clone();
    let player_sender_for_chat = player.sender().clone();
    let chat_sender = chat_sender.clone();
    let (mut chat_receiver, chat_stopper) = Pause::new(chat_receiver);
    // send chat messages
    task::spawn(async move {
        while let Some(next_chat) = chat_receiver.next().await {
            match next_chat {
                Next::Paused(_) => {
                    break;
                }
                Next::Msg(msg) => {
                    let _ = player_sender_for_chat.send(Responses::ChatMessage(msg)).await;
                }
            }
        }
    });
    // handle session response and player commands
    task::spawn(async move {
        let mut player = player.fuse();
        let mut session = session_rsp.fuse();
        loop {
            let next_step = select! {
                cmd = player.next() => {
                    handle_command(cmd, &command, &chat_sender).await
                }
                rsp = session.next() => {
                    handle_session_response(rsp, &player_sender).await
                }
            } as NextStep;
            match next_step {
                NextStep::EnterLobby => {
                    chat_stopper.pause();
                    break ExitState::EnterLobby(player.into_inner())
                },
                NextStep::ExitGame => {
                    chat_stopper.pause();
                    break ExitState::ExitGame
                },
                NextStep::Continue => {},
            }
        }
    })
}

enum NextStep {
    EnterLobby,
    ExitGame,
    Continue,
}

async fn handle_command(
    msg: Option<Received<Messages>>,
    command: &Commands,
    chat_sender: &Sender<String>
) -> NextStep {
    if let Some(msg) = msg {
        match msg {
            Received::Response(rsp) => match rsp {
                Messages::Play(x, y) => command.play(x, y).await,
                Messages::RequestUndo => command.request_undo().await,
                Messages::ApproveUndo => command.approve_undo().await,
                Messages::RejectUndo => command.reject_undo().await,
                Messages::ChatMessage(msg) => {
                    let _ = chat_sender.send(msg).await;
                },
                Messages::QuitGameSession => {
                    command.quit(PlayerQuitReason::QuitSession).await;
                    return NextStep::EnterLobby;
                },
                Messages::ExitGame => {
                    command.quit(PlayerQuitReason::ExitGame).await;
                    return NextStep::ExitGame;
                },
                Messages::ClientError(e) => {
                    command.quit(PlayerQuitReason::Error(e)).await;
                    return NextStep::ExitGame;
                },
                _ => {}
            },
            Received::Ping => {}
            Received::Error(_) | Received::RemoteError(_) => {
                command
                    .quit(PlayerQuitReason::Error("connection error".to_string()))
                    .await;
                return NextStep::ExitGame;
            }
        };
        NextStep::Continue
    } else {
        command.quit(PlayerQuitReason::Disconnected).await;
        NextStep::ExitGame
    }
}

// do not need to handle disconnection at sending message
async fn handle_session_response(rsp: Option<PlayerResponse>, player_sender: &Sender<Responses>) -> NextStep {
    match rsp {
        Some(rsp) => {
            let _ = match rsp {
                PlayerResponse::FieldUpdate(f) => player_sender.send(Responses::FieldUpdate(f)).await,
                PlayerResponse::UndoRequest => player_sender.send(Responses::UndoRequest).await,
                PlayerResponse::Undo(u_rsp) => match u_rsp {
                    UndoResponse::TimeoutRejected => {
                        player_sender.send(Responses::UndoTimeoutRejected).await
                    }
                    UndoResponse::Undo(f) => player_sender.send(Responses::Undo(f)).await,
                    UndoResponse::RejectedByOpponent => {
                        player_sender.send(Responses::UndoRejectedByOpponent).await
                    }
                    UndoResponse::AutoRejected => player_sender.send(Responses::UndoAutoRejected).await,
                },
                PlayerResponse::Quit(q) => {
                    return match q {
                        GameQuitResponse::GameEnd(end) => {
                            let _ = match end {
                                GameResult::BlackTimeout => {
                                    player_sender.send(Responses::GameEndBlackTimeout).await
                                }
                                GameResult::WhiteTimeout => {
                                    player_sender.send(Responses::GameEndWhiteTimeout).await
                                }
                                GameResult::BlackWins => player_sender.send(Responses::GameEndBlackWins).await,
                                GameResult::WhiteWins => player_sender.send(Responses::GameEndWhiteWins).await,
                                GameResult::Draw => player_sender.send(Responses::GameEndDraw).await,
                            };
                            NextStep::EnterLobby
                        },
                        GameQuitResponse::PlayerQuitSession(_) => {
                            let _ = player_sender.send(Responses::OpponentQuitGameSession).await;
                            // enter lobby on opponent quit
                            NextStep::EnterLobby
                        }
                        GameQuitResponse::PlayerExitGame(_) => {
                            let _ = player_sender.send(Responses::OpponentExitGame).await;
                            NextStep::EnterLobby
                        }
                        GameQuitResponse::PlayerDisconnected(_) => {
                            let _ = player_sender.send(Responses::OpponentDisconnected).await;
                            NextStep::EnterLobby
                        }
                        GameQuitResponse::PlayerError(_, e) => {
                            let _ = player_sender
                                .send(Responses::GameSessionError(format!("player error: {}", e)))
                                .await;
                            NextStep::EnterLobby
                        }
                        GameQuitResponse::GameError(e) => {
                            let _ = player_sender
                                .send(Responses::GameSessionError(format!("game error: {}", e)))
                                .await;
                            NextStep::ExitGame
                        }
                    }
                }
            };
            NextStep::Continue
        }
        None => unreachable!()
    }
}
