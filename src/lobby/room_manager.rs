use crate::lobby::client_connection::ClientConnection;
use crate::lobby::messages::{Messages, Responses};
use crate::lobby::room::Room;
use crate::lobby::token::RoomToken;
use async_std::sync::Mutex;
use async_std::task;
use futures::StreamExt;
use log::{info, warn};
use rand::thread_rng;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

const CLEAN_INTERVAL: Duration = Duration::from_secs(30);
const ROOM_LIFE_LENGTH: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct RoomManager {
    rooms: Arc<Mutex<HashMap<RoomToken, Room>>>,
    counter: Arc<AtomicU64>,
}

impl RoomManager {
    pub fn new() -> Self {
        let manager = Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(AtomicU64::default()),
        };
        let manager_clone = manager.clone();
        task::spawn(async move {
            loop {
                task::sleep(CLEAN_INTERVAL).await;
                manager_clone.run_cleaner(ROOM_LIFE_LENGTH).await;
            }
        });
        manager
    }

    pub fn accept_connection(&self, mut conn: ClientConnection) {
        let manager = self.clone();
        task::spawn(async move {
            while let Some(msg) = conn.next().await {
                match msg {
                    Messages::CreateRoom(config) => {
                        let mut rooms = manager.rooms.lock().await;
                        let counter = manager.counter.clone();
                        loop {
                            let token = RoomToken::random(&mut thread_rng());
                            match rooms.entry(token.clone()) {
                                Entry::Occupied(_) => {}
                                Entry::Vacant(e) => {
                                    let room = Room::empty(
                                        token.clone(),
                                        config,
                                        counter,
                                        manager.clone(),
                                    );
                                    let _ = conn
                                        .sender()
                                        .send(Responses::RoomCreated(token.as_code()))
                                        .await;
                                    let _ = room.join(conn).await;
                                    e.insert(room);
                                    break;
                                }
                            }
                        }
                        break;
                    }
                    Messages::SearchOnlinePlayers(name, n) => {
                        let names = conn.get_online_players(name, n as usize).await;
                        let _ = conn.sender().send(Responses::PlayerList(names)).await;
                    }
                    Messages::JoinRoom(token) => {
                        let rooms = manager.rooms.lock().await;
                        if let Some(room) = rooms.get(&token) {
                            match room.join(conn).await {
                                Ok(_) => break,
                                Err(conn_returned) => conn = conn_returned,
                            }
                        } else {
                            let _ = conn
                                .sender()
                                .send(Responses::JoinRoomFailureTokenNotFound)
                                .await;
                        }
                    }
                    Messages::ExitGame => break,
                    Messages::ClientError(e) => {
                        warn!(
                            "player ({}: {}) quit on client error {}",
                            conn.player_name(),
                            conn.player_id(),
                            e
                        );
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    // clean rooms
    async fn run_cleaner(&self, threshold: Duration) {
        let mut rooms = self.rooms.lock().await;
        let mut to_clean = Vec::new();
        for (k, r) in rooms.iter() {
            if let Some(t) = r.inactive_since().await {
                if t.elapsed() > threshold {
                    to_clean.push(k.clone());
                }
            }
        }
        info!("{} room cleaned", to_clean.len());
        for k in to_clean.iter() {
            rooms.remove(k);
        }
    }
}
