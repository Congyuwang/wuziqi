//! This module contains an async safe implementation of wuziqi Game
//!
//! detailed documentation is yet to be Done
//! TODO: documentation
mod game_field;
mod session;

pub use game_field::{compress_field, decompress_field, Color, State};
pub use session::{
    new_session, Commands, FieldState, FieldStateNullable, GameQuitResponse, GameResult,
    PlayerQuitReason, PlayerResponse, SessionConfig, UndoResponse,
};

#[cfg(test)]
mod test_game {
    use crate::game::Color::{Black, White};
    use crate::game::{
        new_session, Color, Commands, PlayerQuitReason, PlayerResponse, SessionConfig,
    };
    use async_std::channel::Receiver;
    use async_std::task;
    use async_std::task::JoinHandle;
    use futures::executor::block_on;
    use futures::future::join3;
    use futures::StreamExt;
    use std::time::Duration;

    fn responses_future(color: Color, mut listener: Receiver<PlayerResponse>) -> JoinHandle<()> {
        task::spawn(async move {
            while let Some(rsp) = listener.next().await {
                println!("Client {:?} received response:\n{:?}", color, rsp)
            }
        })
    }

    async fn play_and_wait(player: &Commands, x: u8, y: u8) {
        println!("play ({x}, {y})");
        player.play(x, y).await;
        task::sleep(Duration::from_millis(100)).await;
    }

    #[test]
    fn test_white_wins() {
        let config = SessionConfig::default();
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 6).await;
            play_and_wait(&white, 5, 7).await;
            play_and_wait(&black, 7, 7).await;
            play_and_wait(&white, 5, 8).await;
            play_and_wait(&black, 8, 8).await;
            play_and_wait(&white, 5, 9).await;
            play_and_wait(&black, 9, 8).await;
            // this should result in white wins and end game
            play_and_wait(&white, 5, 10).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_black_wins() {
        let config = SessionConfig::default();
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 6).await;
            play_and_wait(&white, 5, 7).await;
            play_and_wait(&black, 7, 7).await;
            play_and_wait(&white, 5, 8).await;
            play_and_wait(&black, 8, 8).await;
            play_and_wait(&white, 5, 9).await;
            // this should result in black wins and end game
            play_and_wait(&black, 9, 9).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_ignore_repeated_request() {
        let config = SessionConfig::default();
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 6).await;
            play_and_wait(&white, 5, 7).await;
            // these three should be ignored
            white.play(5, 8).await;
            white.play(5, 9).await;
            white.play(5, 10).await;
            play_and_wait(&black, 7, 7).await;
            play_and_wait(&white, 5, 8).await;
            play_and_wait(&black, 8, 8).await;
            play_and_wait(&white, 5, 9).await;
            // this should result in black wins and end game
            play_and_wait(&black, 9, 9).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_quit_game() {
        let config = SessionConfig::default();
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 6).await;
            play_and_wait(&white, 5, 7).await;
            white.quit(PlayerQuitReason::QuitSession).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_undo_approve_game() {
        let config = SessionConfig::default();
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            white.request_undo().await;
            task::sleep(Duration::from_millis(100)).await;
            black.approve_undo().await;
            task::sleep(Duration::from_millis(100)).await;
            // play after undo
            play_and_wait(&white, 6, 5).await;
            task::sleep(Duration::from_millis(100)).await;
            black.quit(PlayerQuitReason::QuitSession).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_undo_reject_game() {
        let config = SessionConfig::default();
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 7).await;
            white.request_undo().await;
            task::sleep(Duration::from_millis(100)).await;
            black.reject_undo().await;
            task::sleep(Duration::from_millis(100)).await;
            // play after undo
            play_and_wait(&black, 6, 5).await;
            task::sleep(Duration::from_millis(100)).await;
            black.quit(PlayerQuitReason::QuitSession).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_white_play_timeout() {
        let mut config = SessionConfig::default();
        config.play_timeout = 1;
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 5).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_approve_timeout() {
        let mut config = SessionConfig::default();
        config.undo_request_timeout = 1;
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 5).await;
            black.request_undo().await;
            task::sleep(Duration::from_millis(1200)).await;
            black.quit(PlayerQuitReason::QuitSession).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }

    #[test]
    fn test_approve_play_timeout_pause() {
        let mut config = SessionConfig::default();
        config.undo_request_timeout = 1;
        config.play_timeout = 1;
        let (mut black, mut white) = new_session(1000, 100, 200, config);
        let rsp_b = responses_future(Black, black.get_listener().unwrap());
        let rsp_w = responses_future(White, white.get_listener().unwrap());
        let actions = task::spawn(async move {
            play_and_wait(&black, 5, 5).await;
            play_and_wait(&white, 5, 6).await;
            play_and_wait(&black, 6, 5).await;
            black.request_undo().await;
            // should undo-request-timeout, but should not play-timeout
            task::sleep(Duration::from_millis(1500)).await;
            play_and_wait(&white, 6, 6).await;
            // should timeout after this
            task::sleep(Duration::from_millis(1500)).await;
            // this quit action is invalid
            white.quit(PlayerQuitReason::QuitSession).await;
        });
        block_on(join3(rsp_b, rsp_w, actions));
    }
}
