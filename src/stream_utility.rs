use async_std::sync::Mutex;
use futures::stream::{FusedStream, Stream};
use futures::{pin_mut, StreamExt};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// `Plug` wraps around a `Stream<T>`, allowing users to cut off a Stream,
/// and retrieving the inner Stream using `UnplugHandle` from the same thread
/// or another thread.
///
/// ## Interface
///
/// `Plug` itself implements `FusedStream` trait. When there is a message,
/// it returns `Ready(Some(_))`. When polling returns `Ready(None)`, it indicates
/// that either the inner Stream is `unplugged` or `terminated`. To differentiate
/// these two states, call `Plug::stream_terminated()` **after** polling `Ready(None)`.
/// If `Plug::stream_terminated()` is `true`, then `Ready(None)` indicates that
/// inner stream has terminated, otherwise the inner stream is unplugged.
pub struct Plug<F> {
    /// `None` after `UnplugHandle` called
    inner: Arc<PlugInner<F>>,
}

/// `UnplugHandle` returned by calling `Plug::new()` allows
/// for retrieving the inner `Stream` by calling `unplug()`.
pub struct UnplugHandle<F> {
    inner: Arc<PlugInner<F>>,
}

struct PlugInner<F> {
    stream_terminated: AtomicBool,
    stream: Mutex<Option<F>>,
    waker: Mutex<Option<Waker>>,
}

impl<F> UnplugHandle<F> {
    /// Terminate `Plug<Stream<>>`, retrieve the inner stream.
    /// It returns `None` if the inner stream has already terminated
    /// to prevent polling again after `Ready<None>`.
    /// Returns:
    /// - `None` if the inner stream has terminated
    /// - Some<Stream> if the inner stream is not yet terminated
    pub async fn unplug(self) -> Option<F> {
        // lock stream during the whole function
        let mut stream = self.inner.stream.lock().await;
        let mut waker = self.inner.waker.lock().await;
        if let Some(waker) = waker.take() {
            waker.wake()
        }
        if self.inner.stream_terminated.load(SeqCst) {
            None
        } else {
            Some(stream.take().unwrap())
        }
    }
}

impl<F, R> Plug<F>
where
    F: Stream<Item = R>,
{
    pub fn new(inner: F) -> (Self, UnplugHandle<F>) {
        let inner = Arc::new(PlugInner {
            stream_terminated: AtomicBool::new(false),
            stream: Mutex::new(Some(inner)),
            waker: Mutex::new(None),
        });
        (
            Plug {
                inner: inner.clone(),
            },
            UnplugHandle { inner },
        )
    }
}

impl<F, R> Stream for Plug<F>
where
    F: Stream<Item = R> + Unpin,
{
    type Item = R;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.stream.try_lock() {
            None => {
                // cannot lock only in the case of `unplug()` being called,
                // so it is certain that `unplug is called`
                Poll::Ready(None)
            }
            // `self.inner.stream` is locked during the whole `Some(_)` block.
            // Therefore,
            Some(mut stream) => {
                match stream.as_mut() {
                    None => {
                        // the stream is already unplugged
                        Poll::Ready(None)
                    }
                    Some(stream) => {
                        match stream.poll_next_unpin(cx) {
                            Poll::Ready(item) => {
                                if item.is_none() {
                                    // record that the stream has terminated
                                    self.inner.stream_terminated.store(true, SeqCst);
                                }
                                Poll::Ready(item)
                            }
                            Poll::Pending => {
                                // in this case, wait for notification from
                                // inner stream and UnplugHandle
                                let waker = {
                                    let waker_lock = self.inner.waker.lock();
                                    pin_mut!(waker_lock);
                                    waker_lock.poll(cx)
                                };
                                if let Poll::Ready(mut waker) = waker {
                                    waker.replace(cx.waker().clone());
                                }
                                Poll::Pending
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<F, R> FusedStream for Plug<F>
where
    F: Stream<Item = R> + Unpin,
{
    fn is_terminated(&self) -> bool {
        match self.inner.stream.try_lock() {
            // unplugging
            None => true,
            Some(stream) => {
                match stream.as_ref() {
                    // unplugged
                    None => true,
                    // inner terminated
                    Some(_) => self.stream_terminated(),
                }
            }
        }
    }
}

impl<F> Plug<F> {
    /// Since `Plug<Stream<T>>` might return `Ready(None)` on two states:
    /// - the inner stream is unplugged
    /// - the inner stream is terminated
    ///
    /// As `Stream::is_terminated()` will return `true` in both states,
    /// we implement an extra method `stream_terminated()`, which return `true`
    /// if and only if the inner stream has terminated on returning `Ready(None)`.
    pub fn stream_terminated(&self) -> bool {
        self.inner.stream_terminated.load(SeqCst)
    }
}

#[cfg(test)]
mod test_stoppable {
    use crate::stream_utility::Plug;
    use async_std::channel::bounded;
    use async_std::task;
    use futures::executor::block_on;
    use futures::stream::FusedStream;
    use futures::StreamExt;

    #[test]
    fn test_early_stop() {
        let (s, r) = bounded::<i32>(10);
        let (mut r, unplug_handle) = Plug::new(r);
        block_on(async move {
            // send five messages
            s.send(0).await.unwrap();
            s.send(1).await.unwrap();
            s.send(2).await.unwrap();
            s.send(3).await.unwrap();
            s.send(4).await.unwrap();
            // receive three messages
            assert!(matches!(r.next().await, Some(0)));
            assert!(matches!(r.next().await, Some(1)));
            assert!(matches!(r.next().await, Some(2)));
            // pause
            let inner = unplug_handle.unplug().await.unwrap();
            let (mut r_new, _) = Plug::new(inner);
            assert!(matches!(r.next().await, None));
            assert!(r.is_terminated());
            assert!(!r.stream_terminated());
            // receive the remaining messages
            assert!(matches!(r_new.next().await, Some(3)));
            assert!(matches!(r_new.next().await, Some(4)));
            drop(s);
            assert!(matches!(r_new.next().await, None));
            assert!(r_new.is_terminated());
        });
    }

    #[test]
    fn test_unplug_while_pending() {
        let (s, r) = bounded::<i32>(10);
        let (mut r, unplug_handle) = Plug::new(r);
        block_on(async move {
            // send five messages
            s.send(0).await.unwrap();
            s.send(1).await.unwrap();
            // receive three messages
            assert!(matches!(r.next().await, Some(0)));
            assert!(matches!(r.next().await, Some(1)));
            let r_next = task::spawn(async move { r.next().await });
            let (mut inner, _) = Plug::new(unplug_handle.unplug().await.unwrap());
            s.send(2).await.unwrap();
            assert!(matches!(inner.next().await, Some(2)));
            assert!(matches!(r_next.await, None));
            assert!(!inner.is_terminated());
            drop(s);
            assert!(matches!(inner.next().await, None));
            assert!(inner.is_terminated());
            assert!(inner.stream_terminated());
        })
    }
}
