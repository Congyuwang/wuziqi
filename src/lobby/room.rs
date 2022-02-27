use crate::game::SessionConfig;
use crate::lobby::client_connection::ClientConnection;
use crate::lobby::game_session::{start_game_session, ExitState, PlayerResult};
use crate::lobby::messages::{Messages, Responses, RoomState};
use crate::lobby::room::Position::{First, Second};
use crate::lobby::room_manager::RoomManager;
use crate::lobby::token::RoomToken;
use crate::stream_utility::{Plug, UnplugHandle};
use crate::CHANNEL_SIZE;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::sync::Mutex;
use async_std::task;
use async_std::task::block_on;
use futures::StreamExt;
use log::{error, info};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::time::Instant;

pub(crate) struct Room {
    inner: Arc<Mutex<RoomInner>>,
}

impl Room {
    pub(crate) fn empty(
        token: RoomToken,
        session_config: SessionConfig,
        session_counter: Arc<AtomicU64>,
        manager: RoomManager,
    ) -> Self {
        Room {
            inner: RoomInner::empty(token, session_config, session_counter, manager),
        }
    }

    pub(crate) async fn join(&self, conn: ClientConnection) -> Result<Position, ClientConnection> {
        self.inner.lock().await.join(conn).await
    }

    pub(crate) async fn inactive_since(&self) -> Option<Instant> {
        self.inner.lock().await.inactive_since
    }
}

/// dropping a `Room`
struct RoomInner {
    // seats mark whether a player is in the room
    token: RoomToken,
    seats: (Option<PlayerInfo>, Option<PlayerInfo>),
    room_msg_sender: Sender<(Position, Messages)>,
    session_config: SessionConfig,
    session_counter: Arc<AtomicU64>,
    // number of winnings
    scores: (u16, u16),
    // stop background message task
    killer: Option<UnplugHandle<Receiver<(Position, Messages)>>>,
    // room lifetime management
    inactive_since: Option<Instant>,
}

impl RoomInner {
    /// create an empty room
    fn empty(
        token: RoomToken,
        session_config: SessionConfig,
        session_counter: Arc<AtomicU64>,
        room_manager: RoomManager,
    ) -> Arc<Mutex<RoomInner>> {
        let inner_channel = bounded(CHANNEL_SIZE);
        let (recv, room_killer) = Plug::new(inner_channel.1);
        let room = Arc::new(Mutex::new(RoomInner {
            token,
            seats: (None, None),
            room_msg_sender: inner_channel.0,
            session_config,
            session_counter,
            scores: (0, 0),
            killer: Some(room_killer),
            inactive_since: Some(Instant::now()),
        }));
        run_room(room.clone(), recv, room_manager);
        room
    }

    /// join a new player
    /// - send join success message to player
    /// - send OpponentJoinRoom to opponent
    /// - start listening to player
    /// - clear session score board
    async fn join(&mut self, conn: ClientConnection) -> Result<Position, ClientConnection> {
        if let Some(pos) = self.empty_position() {
            let room_state = match self.player_info(pos.opponent()) {
                None => RoomState::Empty,
                Some(info) => {
                    if info.is_ready() {
                        RoomState::OpponentReady(info.player_name.clone())
                    } else {
                        RoomState::OpponentUnready(info.player_name.clone())
                    }
                }
            };
            let (player_info, conn) = PlayerInfo::new(conn);
            info!(
                "player {} join room {}",
                player_info.player_id,
                self.token.as_code()
            );
            self.run_player_message_loop(conn, pos);
            let my_name = player_info.player_name.clone();
            self.player_info_mut(pos).replace(player_info);
            self.clear_score();
            self.inactive_since = None;
            let _ = self
                .send_response(
                    pos,
                    Responses::JoinRoomSuccess(self.token.as_code(), room_state),
                )
                .await;
            let _ = self
                .send_response(pos.opponent(), Responses::OpponentJoinRoom(my_name))
                .await;
            Ok(pos)
        } else {
            let _ = conn.sender().send(Responses::JoinRoomFailureRoomFull).await;
            Err(conn)
        }
    }

    fn player_scored(&mut self, pos: Position) {
        let score = match pos {
            First => &mut self.scores.0,
            Second => &mut self.scores.1,
        };
        *score += 1;
    }

    /// when both players have returned from game session.
    /// call this only when both seats are `Some`
    ///
    /// update score responses
    async fn join_both_on_session_return(
        &mut self,
        conn1: ClientConnection,
        conn2: ClientConnection,
    ) {
        let p1_info = self.player_info_mut(First).as_mut().unwrap();
        let conn1 = p1_info.return_from_session(conn1);
        let p1_name = p1_info.player_name.clone();
        p1_info.unready();
        let p2_info = self.player_info_mut(Second).as_mut().unwrap();
        let conn2 = p2_info.return_from_session(conn2);
        let p2_name = p2_info.player_name.clone();
        p2_info.unready();
        self.run_player_message_loop(conn1, First);
        self.run_player_message_loop(conn2, Second);
        // send responses on returning to room
        self.send_response(
            First,
            Responses::JoinRoomSuccess(
                self.token.as_code(),
                RoomState::OpponentUnready(p2_name.clone()),
            ),
        )
        .await;
        self.send_response(
            Second,
            Responses::JoinRoomSuccess(
                self.token.as_code(),
                RoomState::OpponentUnready(p1_name.clone()),
            ),
        )
        .await;
        let score_rsp = Responses::RoomScores((p1_name, self.scores.0), (p2_name, self.scores.1));
        self.send_response(First, score_rsp.clone()).await;
        self.send_response(Second, score_rsp).await;
    }

    /// this function does not deal with score boards
    async fn join_single_on_session_return(&mut self, exit_state: ExitState, pos: Position) {
        if let ExitState::ReturnRoom(conn, _) = exit_state {
            let p_info = self.player_info_mut(pos).as_mut().unwrap();
            let conn = p_info.return_from_session(conn);
            p_info.unready();
            self.run_player_message_loop(conn, pos);
            self.send_response(
                pos,
                Responses::JoinRoomSuccess(self.token.as_code(), RoomState::Empty),
            )
            .await
        } else {
            self.exit(pos).await;
        }
    }

    async fn chat(&mut self, pos: Position, message: String) {
        if let Some(info) = self.player_info(pos) {
            let name = info.player_name.clone();
            let _ = info
                .sender
                .send(Responses::ChatMessage(name, message))
                .await;
        }
    }

    /// return unplug handles if both ready
    async fn ready(
        &mut self,
        pos: Position,
    ) -> Option<(
        UnplugHandle<ClientConnection>,
        UnplugHandle<ClientConnection>,
    )> {
        let both_ready = if let Some(info_1) = self.player_info_mut(pos) {
            info_1.ready();
            // if opponent is ready
            if let Some(info_2) = self.player_info(pos.opponent()) {
                let _ = info_2.sender.send(Responses::OpponentReady).await;
                info_2.is_ready()
            } else {
                false
            }
        } else {
            false
        };
        if both_ready {
            Some((
                self.player_info_mut(First)
                    .as_mut()
                    .unwrap()
                    .start_session()
                    .unwrap(),
                self.player_info_mut(Second)
                    .as_mut()
                    .unwrap()
                    .start_session()
                    .unwrap(),
            ))
        } else {
            None
        }
    }

    async fn unready(&mut self, pos: Position) {
        let info = self.player_info_mut(pos).as_mut().unwrap();
        info.unready();
        let _ = self
            .send_response(pos.opponent(), Responses::OpponentUnready)
            .await;
    }

    async fn exit(&mut self, pos: Position) -> Option<ClientConnection> {
        let _ = self
            .send_response(pos.opponent(), Responses::OpponentQuitRoom)
            .await;
        self.clear_score();
        let mut info = self.player_info_mut(pos).take()?;
        if let (None, None) = self.seats {
            self.inactive_since = Some(Instant::now());
        }
        info.unplug_handle.take()?.unplug().await
    }

    /// internal function for sending responses to a player
    async fn send_response(&self, pos: Position, rsp: Responses) {
        let seat = self.player_info(pos);
        if let Some(info) = seat {
            let _ = info.sender.send(rsp).await;
        }
    }

    /// clear score board
    fn clear_score(&mut self) {
        self.scores = (0, 0);
    }

    /// receive messages from player, on accidental disconnection send `ExitGame`
    fn run_player_message_loop(&self, mut conn: Plug<ClientConnection>, pos: Position) {
        let sender = self.room_msg_sender.clone();
        task::spawn(async move {
            while let Some(msg) = conn.next().await {
                if sender.send((pos, msg)).await.is_err() {
                    break;
                }
            }
            // disconnection case: send ExitGame message
            if conn.stream_terminated() {
                let _ = sender.send((pos, Messages::ExitGame)).await;
            }
        });
    }

    fn player_info(&self, pos: Position) -> &Option<PlayerInfo> {
        match pos {
            First => &self.seats.0,
            Second => &self.seats.1,
        }
    }

    /// get player info of a position
    fn player_info_mut(&mut self, pos: Position) -> &mut Option<PlayerInfo> {
        match pos {
            First => &mut self.seats.0,
            Second => &mut self.seats.1,
        }
    }

    /// return the first empty position, `None` if the room is full
    fn empty_position(&self) -> Option<Position> {
        match self.seats {
            (None, _) => Some(First),
            (Some(_), None) => Some(Second),
            (Some(_), Some(_)) => None,
        }
    }
}

// the background task of `Room`, kill once the Room is dropped
fn run_room(
    room: Arc<Mutex<RoomInner>>,
    mut recv: Plug<Receiver<(Position, Messages)>>,
    room_manager: RoomManager,
) {
    task::spawn(async move {
        while let Some((pos, msg)) = recv.next().await {
            match msg {
                Messages::Ready => {
                    on_player_ready(&room, pos).await;
                }
                Messages::Unready => {
                    room.lock().await.unready(pos).await;
                }
                Messages::ChatMessage(msg) => {
                    room.lock().await.chat(pos.opponent(), msg).await;
                }
                Messages::QuitRoom => {
                    if let Some(conn) = room.lock().await.exit(pos).await {
                        room_manager.accept_connection(conn);
                    }
                }
                Messages::ExitGame | Messages::ClientError(_) => {
                    room.lock().await.exit(pos).await;
                }
                _ => {}
            }
        }
    });
}

async fn on_player_ready(room: &Arc<Mutex<RoomInner>>, pos: Position) {
    let ready_result = room.lock().await.ready(pos).await;
    if let Some((conn1, conn2)) = ready_result {
        // when player connection ended, player_message_loop will send `QuitRoom` command
        let (conn1, conn2) = (conn1.unplug().await.unwrap(), conn2.unplug().await.unwrap());
        // randomly assign colors
        let is_p1_black = rand::random::<bool>();
        let (b_conn, w_conn) = if is_p1_black {
            (conn1, conn2)
        } else {
            (conn2, conn1)
        };
        let s_id = room.lock().await.session_counter.fetch_add(1, SeqCst);
        let s_config = room.lock().await.session_config.clone();
        let b_id = b_conn.player_id();
        let w_id = w_conn.player_id();
        let (b_exit, w_exit) = start_game_session(s_id, b_id, w_id, s_config, b_conn, w_conn).await;
        let (exit1, exit2) = if is_p1_black {
            (b_exit, w_exit)
        } else {
            (w_exit, b_exit)
        };
        match (exit1, exit2) {
            (ExitState::ReturnRoom(conn1, result1), ExitState::ReturnRoom(conn2, result2)) => {
                // update score board
                match (result1, result2) {
                    (PlayerResult::Win, PlayerResult::Lose)
                    | (PlayerResult::OpponentQuit, PlayerResult::Quit) => {
                        room.lock().await.player_scored(First);
                    }
                    (PlayerResult::Lose, PlayerResult::Win)
                    | (PlayerResult::Quit, PlayerResult::OpponentQuit) => {
                        room.lock().await.player_scored(Second);
                    }
                    (PlayerResult::Draw, PlayerResult::Draw) => {}
                    (result1, result2) => {
                        error!("game session end in bad state (p1: {result1}, p2: {result2})");
                    }
                }
                room.lock()
                    .await
                    .join_both_on_session_return(conn1, conn2)
                    .await;
            }
            (exit1, exit2) => {
                // one of the players quit, do not need to update score board
                room.lock()
                    .await
                    .join_single_on_session_return(exit1, First)
                    .await;
                room.lock()
                    .await
                    .join_single_on_session_return(exit2, Second)
                    .await;
            }
        }
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        if let Some(killer) = block_on(self.inner.lock()).killer.take() {
            block_on(killer.unplug());
        }
    }
}

// player info records information about a player
struct PlayerInfo {
    player_name: String,
    player_id: u64,
    sender: Sender<Responses>,
    unplug_handle: Option<UnplugHandle<ClientConnection>>,
    ready: bool,
}

impl PlayerInfo {
    fn new(conn: ClientConnection) -> (Self, Plug<ClientConnection>) {
        let player_name = conn.player_name().to_string();
        let player_id = conn.player_id();
        let sender = conn.sender().clone();
        let (plug, unplug) = Plug::new(conn);
        (
            PlayerInfo {
                player_name,
                player_id,
                sender,
                unplug_handle: Some(unplug),
                ready: false,
            },
            plug,
        )
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn ready(&mut self) {
        self.ready = true;
    }

    fn unready(&mut self) {
        self.ready = false;
    }

    /// this function will turn `unplug_handle` into `None`
    fn start_session(&mut self) -> Option<UnplugHandle<ClientConnection>> {
        self.unplug_handle.take()
    }

    fn return_from_session(&mut self, conn: ClientConnection) -> Plug<ClientConnection> {
        let (plug, unplug) = Plug::new(conn);
        self.unplug_handle.replace(unplug);
        plug
    }
}

// position of seats in the room
#[derive(Clone, Copy)]
pub(crate) enum Position {
    First,
    Second,
}

impl Position {
    fn opponent(&self) -> Position {
        match self {
            First => Second,
            Second => First,
        }
    }
}
