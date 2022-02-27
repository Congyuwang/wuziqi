use anyhow::Result;
use async_std::channel::{SendError, Sender};
use async_std::sync::Mutex;
use async_std::task;
use std::borrow::BorrowMut;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// This `TimeoutGet` accept a `Sender`, and sends a `timeout_msg`
/// after certain `delay`, like an alarm.
///
/// When `send` method is called, this `TimeoutGate` is consumed.
///
/// All `Sender` instances inside `TimeoutGate` will be dropped
/// once the `timeout_msg` is sent, or `msg` is sent,
/// or if `TimeoutGate` is paused and dropped.
pub(crate) struct TimeoutGate<T> {
    /// needed for pausing
    time: Instant,
    total_elapsed: Duration,
    total_delay: Option<Duration>,
    msg_timeout: Arc<Mutex<Option<T>>>,
    state: Arc<Mutex<State>>,
    sender: Arc<Mutex<Option<Sender<T>>>>,
}

#[derive(Debug)]
enum State {
    Waiting(usize),
    Paused(usize),
    MessageSent,
    TimeoutSent,
}

impl<T: Send + 'static> TimeoutGate<T> {
    pub(crate) fn new(total_delay: Option<Duration>, sender: Sender<T>, timeout_msg: T) -> Self {
        let mut gate = TimeoutGate {
            time: Instant::now(),
            total_elapsed: Duration::new(0, 0),
            state: Arc::new(Mutex::new(State::Waiting(0))),
            msg_timeout: Arc::new(Mutex::new(Some(timeout_msg))),
            total_delay,
            sender: Arc::new(Mutex::new(Some(sender))),
        };
        let total_delay = gate.total_delay.clone();
        TimeoutGate::fire_alarm(&mut gate, total_delay, 0);
        gate
    }

    /// can be used to send message only once
    pub(crate) async fn send(self, msg: T) -> Result<(), SendError<T>> {
        let mut state = self.state.lock().await;
        // send only in waiting state
        if matches!(state.deref(), State::Waiting(_)) {
            *state = State::MessageSent;
            let mut sender = self.sender.lock().await;
            sender.take().unwrap().send(msg).await?
        }
        Ok(())
    }

    /// if not yet sent or alarmed, may pause
    pub(crate) async fn pause(&mut self) {
        let mut state = self.state.lock().await;
        // pause only in waiting state
        let seq = if let State::Waiting(seq) = state.deref_mut() {
            // update total_elapsed
            self.total_elapsed += self.time.elapsed();
            self.time = Instant::now();
            // invalidate previous timeout alarm
            *seq += 1;
            *seq
        } else {
            return ();
        };
        *state = State::Paused(seq);
    }

    /// if pause is successful, calling resume with extra time
    /// (compensate for the inconvenience of pausing).
    pub(crate) async fn resume(&mut self, extra_time: Duration) {
        let mut state = self.state.lock().await;
        // resume only in pause
        let (seq, delay) = if let State::Paused(seq) = state.deref() {
            match &mut self.total_delay {
                None => {
                    // start no alarm, wait forever mode
                    (*seq, None)
                }
                Some(total_delay) => {
                    // compute how much more time is allowed for this resume run
                    *total_delay += extra_time;
                    let delay = total_delay.saturating_sub(self.total_elapsed);
                    (*seq, Some(delay))
                }
            }
        } else {
            return ();
        };
        *state = State::Waiting(seq);
        drop(state);
        self.fire_alarm(delay, seq);
    }

    /// sleep for sometime and send Timeout message
    /// this may be called only once
    ///
    /// if `delay` is None, the alarm is not fired.
    fn fire_alarm(&mut self, delay: Option<Duration>, seq: usize) {
        match delay {
            None => {}
            Some(delay) => {
                let state = self.state.clone();
                let sender = self.sender.clone();
                let msg_timeout = self.msg_timeout.clone();
                task::spawn(async move {
                    task::sleep(delay).await;
                    let mut state = state.lock().await;
                    if let State::Waiting(s) = state.deref() {
                        if *s == seq {
                            let mut msg_timeout = msg_timeout.lock().await;
                            let mut sender = sender.lock().await;
                            let _ = sender
                                .take()
                                .unwrap()
                                .send(msg_timeout.borrow_mut().take().unwrap())
                                .await;
                        }
                        *state = State::TimeoutSent;
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod test_timeout {
    use super::*;
    use async_std::channel::bounded;
    use futures::executor::block_on;
    use futures::StreamExt;

    #[test]
    fn send_timeout() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let _ = TimeoutGate::new(
            Some(Duration::from_millis(100)),
            msg_sender,
            "timeout".to_string(),
        );
        block_on(async {
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, "timeout".to_string())
            }
        })
    }

    #[test]
    fn send_not_timeout() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let gate = TimeoutGate::new(Some(Duration::from_millis(1000)), msg_sender, 0);
        block_on(async {
            task::sleep(Duration::from_millis(100)).await;
            gate.send(1).await.unwrap();
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 1)
            }
        })
    }

    #[test]
    fn send_never_timeout() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let gate = TimeoutGate::new(None, msg_sender, 0);
        block_on(async {
            // without sending this message, this code will block forever
            gate.send(1).await.unwrap();
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 1)
            }
        })
    }

    #[test]
    fn pause_no_resume() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let mut gate = TimeoutGate::new(Some(Duration::from_millis(100)), msg_sender, 0);
        block_on(async {
            // pause alarm
            gate.pause().await;
            task::sleep(Duration::from_millis(500)).await;
            // send in pausing state is ignored
            gate.send(1).await.unwrap();
            // Sender owned by gate will be released upon sending timeout_msg
            assert!(matches!(msg_receiver.next().await, None));
        })
    }

    #[test]
    fn pause_resume() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let mut gate = TimeoutGate::new(Some(Duration::from_millis(100)), msg_sender, 0);
        block_on(async {
            // pause alarm
            gate.pause().await;
            task::sleep(Duration::from_millis(500)).await;
            gate.resume(Duration::new(0, 0)).await;
            // should send successfully
            gate.send(1).await.unwrap();
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 1)
            }
        })
    }

    #[test]
    fn pause_resume_timeout() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let mut gate = TimeoutGate::new(Some(Duration::from_millis(100)), msg_sender, 0);
        block_on(async {
            // pause alarm
            gate.pause().await;
            task::sleep(Duration::from_millis(500)).await;
            gate.resume(Duration::new(0, 0)).await;
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 0)
            }
        })
    }

    #[test]
    fn pause_resume_timeout_send() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let mut gate = TimeoutGate::new(Some(Duration::from_millis(100)), msg_sender, 0);
        block_on(async {
            // pause alarm
            gate.pause().await;
            // sleep longer than Timeout parameter
            task::sleep(Duration::from_millis(500)).await;
            gate.resume(Duration::new(0, 0)).await;
            task::sleep(Duration::from_millis(200)).await;
            // this send takes too long, therefore timeout and receives 0
            gate.send(1).await.unwrap();
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 0)
            }
        })
    }

    #[test]
    fn pause_resume_timeout_send_extra_time() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let mut gate = TimeoutGate::new(Some(Duration::from_millis(100)), msg_sender, 0);
        block_on(async {
            // pause alarm
            gate.pause().await;
            // sleep longer than Timeout parameter
            task::sleep(Duration::from_millis(500)).await;
            // with 150 ms extra, this should send successfully
            gate.resume(Duration::from_millis(500)).await;
            task::sleep(Duration::from_millis(200)).await;
            gate.send(1).await.unwrap();
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 1)
            }
        })
    }

    #[test]
    fn multiple_pause_resume_timeout() {
        let (msg_sender, mut msg_receiver) = bounded(1);
        let mut gate = TimeoutGate::new(Some(Duration::from_millis(100)), msg_sender, 0);
        block_on(async {
            // these resume are ignored
            gate.resume(Duration::new(0, 0)).await;
            gate.resume(Duration::new(0, 0)).await;
            // pause alarm
            gate.pause().await;
            // pause below are ignored
            gate.pause().await;
            gate.pause().await;
            task::sleep(Duration::from_millis(500)).await;
            gate.resume(Duration::new(0, 0)).await;
            // resume below are ignored
            gate.resume(Duration::new(0, 0)).await;
            gate.resume(Duration::new(0, 0)).await;
            // even though
            task::sleep(Duration::from_millis(200)).await;
            gate.send(1).await.unwrap();
            while let Some(msg) = msg_receiver.next().await {
                assert_eq!(msg, 0)
            }
        })
    }
}
