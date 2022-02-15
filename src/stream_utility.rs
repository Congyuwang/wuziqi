use std::future::Future;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{Context, Poll};
use async_std::channel::{bounded, Receiver, Sender};
use async_std::stream::Stream;
use async_std::task;
use futures::channel::oneshot;
use futures::{FutureExt, pin_mut, select, StreamExt};
use futures::channel::oneshot::Canceled;
use futures::stream::FusedStream;

/// a wrapper around Receiver which can be stopped and got the receiver back
pub struct Pause<F> {
    inner: Option<F>,
    stopper_r: Option<oneshot::Receiver<()>>,
}

pub struct Stopper {
    stopper_s: oneshot::Sender<()>,
}

impl Stopper {
    fn stop(self) {
        let _ = self.stopper_s.send(());
    }
}

/// Might return `Paused` if paused before terminated
pub enum Next<F, R> {
    Paused(F),
    Msg(R),
}

impl<F, R> Pause<F>
    where F: Stream<Item=R>
{
    pub fn new(mut inner: F) -> (Self, Stopper) {
        let (stopper_s, stopper_r) = oneshot::channel::<()>();
        (
            Pause { inner: Some(inner), stopper_r: Some(stopper_r), },
            Stopper { stopper_s }
        )
    }
}

impl<F, R> Stream for Pause<F>
    where F: Stream<Item=R> + Unpin
{
    type Item = Next<F, R>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.inner.is_none() {
            return Poll::Ready(None)
        }
        if self.stopper_r.is_some() {
            if let Poll::Ready(o) = self.stopper_r.as_mut().unwrap().poll_unpin(cx) {
                match o {
                    Ok(_) => {
                        return Poll::Ready(Some(Next::Paused(self.inner.take().unwrap())))
                    },
                    Err(_) => {
                        // drop self.stopper_r if stopper cancelled
                        drop(self.stopper_r.take());
                        // continue to poll_next
                    }
                }
            }
        }
        match self.inner.as_mut().unwrap().poll_next_unpin(cx) {
            Poll::Ready(o) => {
                match o {
                    None => Poll::Ready(None),
                    Some(msg) => Poll::Ready(Some(Next::Msg(msg))),
                }
            }
            Poll::Pending => Poll::Pending
        }
    }
}

#[cfg(test)]
mod test_stoppable {
    use async_std::channel::bounded;
    use futures::executor::block_on;
    use futures::stream::FusedStream;
    use futures::StreamExt;
    use crate::stream_utility::{Next, Pause};

    #[test]
    fn test_early_stop() {
        let (s, r) = bounded::<i32>(10);
        let (r, stopper) = Pause::new(r);
        let mut r = r.fuse();
        block_on(async move {
            s.send(0).await.unwrap();
            s.send(1).await.unwrap();
            s.send(2).await.unwrap();
            assert!(matches!(r.next().await, Some(Next::Msg(0))));
            assert!(matches!(r.next().await, Some(Next::Msg(1))));
            assert!(matches!(r.next().await, Some(Next::Msg(2))));
            stopper.stop();
            let mut r_new = match r.next().await {
                Some(Next::Paused(r_new)) => r_new,
                _ => panic!("should stop after calling `Stop::stop()`")
            };
            s.send(3).await.unwrap();
            s.send(4).await.unwrap();
            assert!(matches!(r.next().await, None));
            assert!(r.is_terminated());
            assert!(matches!(r_new.next().await, Some(3)));
            assert!(matches!(r_new.next().await, Some(4)));
            drop(s);
            assert!(matches!(r_new.next().await, None));
            assert!(r_new.is_terminated());
        });
    }
}
