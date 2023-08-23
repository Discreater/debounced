use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::{FutureExt, Stream, StreamExt};

use super::{delayed, Delayed};

/// Stream that delays its items for a given duration and only yields the most
/// recent item afterwards.
///
/// ```rust
/// # use std::time::{Duration, Instant};
/// # use futures_util::{SinkExt, StreamExt};
/// # tokio_test::block_on(async {
/// use debounced::Debounced;
///
/// # let start = Instant::now();
/// let (mut sender, receiver) = futures_channel::mpsc::channel(1024);
/// let mut debounced = Debounced::new(receiver, Duration::from_secs(1));
/// sender.send(21).await;
/// sender.send(42).await;
/// assert_eq!(debounced.next().await, Some(42));
/// assert_eq!(start.elapsed().as_secs(), 1);
/// std::mem::drop(sender);
/// assert_eq!(debounced.next().await, None);
/// # })
pub struct Debounced<S>
where
    S: Stream,
{
    stream: S,
    delay: Duration,
    pending: Option<Delayed<S::Item>>,
    direct_if: Option<fn(&S::Item) -> bool>,
    max_waiting: Option<MaxWaiting>,
}

impl<S> Debounced<S>
where
    S: Stream + Unpin,
{
    /// Returns a new stream that delays its items for a given duration and only
    /// yields the most recent item afterwards.
    pub fn new(
        stream: S,
        delay: Duration,
        condition: Option<fn(&S::Item) -> bool>,
        max_waiting: Option<Duration>,
    ) -> Debounced<S> {
        Debounced {
            stream,
            delay,
            pending: None,
            direct_if: condition,
            max_waiting: max_waiting.map(|max_waiting| MaxWaiting {
                duration: max_waiting,
                first_call: Instant::now(),
            }),
        }
    }
}

struct MaxWaiting {
    duration: Duration,
    first_call: Instant,
}

impl<S> Stream for Debounced<S>
where
    S: Stream + Unpin,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Poll::Ready(next) = self.stream.poll_next_unpin(cx) {
            match next {
                Some(next) => {
                    if self.direct_if.as_ref().is_some_and(|c| c(&next)) {
                        return Poll::Ready(Some(next));
                    } else {
                        #[allow(unused_variables, unused_mut)]
                        if let Some(mut max_waiting) = self.max_waiting.take() {
                            unimplemented!("Implementation below is wrong.");
                            #[allow(unreachable_code)]
                            if let Some(pending) = self.pending.take() {
                                // not first item
                                if Instant::now() - max_waiting.first_call > max_waiting.duration {
                                    // max waiting time exceeded, interrupt current pending and return it's value.
                                    // We can ensure that the pending have not been polled.
                                    let value = pending.caesarean_birth().unwrap();
                                    return Poll::Ready(Some(value));
                                } else {
                                    // max waiting time not exceeded
                                    self.pending = Some(delayed(next, self.delay));
                                }
                            } else {
                                // first item
                                self.pending = Some(delayed(next, self.delay));
                                max_waiting.first_call = Instant::now();
                                self.max_waiting = Some(max_waiting);
                            }
                        } else {
                            self.pending = Some(delayed(next, self.delay))
                        }
                    }
                }
                None => {
                    if self.pending.is_none() {
                        return Poll::Ready(None);
                    }
                    break;
                }
            }
        }

        match self.pending.as_mut() {
            Some(pending) => match pending.poll_unpin(cx) {
                Poll::Ready(value) => {
                    let _ = self.pending.take();
                    Poll::Ready(Some(value))
                }
                Poll::Pending => Poll::Pending,
            },
            None => Poll::Pending,
        }
    }
}

/// Returns a new stream that delays its items for a given duration and only
/// yields the most recent item afterwards.
///
/// ```rust
/// # use std::time::{Duration, Instant};
/// # use futures_util::{SinkExt, StreamExt};
/// # tokio_test::block_on(async {
/// use debounced::debounced;
///
/// # let start = Instant::now();
/// let (mut sender, receiver) = futures_channel::mpsc::channel(1024);
/// let mut debounced = debounced(receiver, Duration::from_secs(1));
/// sender.send(21).await;
/// sender.send(42).await;
/// assert_eq!(debounced.next().await, Some(42));
/// assert_eq!(start.elapsed().as_secs(), 1);
/// std::mem::drop(sender);
/// assert_eq!(debounced.next().await, None);
/// # })
pub fn debounced<S>(stream: S, delay: Duration) -> Debounced<S>
where
    S: Stream + Unpin,
{
    Debounced::new(stream, delay, None, None)
}

/// If the stream item statisfies the condition, the item is delayed for a given duration and only yields the most recent item afterwards.
/// Otherwise, the item is yielded immediately.
///
/// ```rust
/// # use std::time::{Duration, Instant};
/// # use futures_util::{SinkExt, StreamExt};
/// # tokio_test::block_on(async {
/// use debounced::debounced_if;
///
/// # let start = Instant::now();
/// let (mut sender, receiver) = futures_channel::mpsc::channel(1024);
/// let mut debounced = debounced_if(receiver, Duration::from_secs(1), |x| x < &30);
/// sender.send(21).await;
/// sender.send(32).await;
/// sender.send(25).await;
/// sender.send(42).await;
/// assert_eq!(debounced.next().await, Some(21));
/// assert_eq!(debounced.next().await, Some(25));
/// assert_eq!(debounced.next().await, Some(42));
/// assert_eq!(start.elapsed().as_secs(), 1);
/// std::mem::drop(sender);
/// assert_eq!(debounced.next().await, None);
/// # })
pub fn debounced_if<S>(stream: S, delay: Duration, condition: fn(&S::Item) -> bool) -> Debounced<S>
where
    S: Stream + Unpin,
{
    Debounced::new(stream, delay, Some(condition), None)
}

/// Builder for [`Debounced`].
///
/// ```rust
/// let debounced = DebouncedBuilder::new()
///     .delay(Duration::from_secs(1))
///     .conditional(|x| x < &30)
///     .build(receiver);
/// ```
pub struct DebouncedBuilder<S>
where
    S: Stream + Unpin,
{
    delay: Duration,
    condition: Option<fn(&S::Item) -> bool>,
    max_waiting: Option<Duration>,
}

impl<S> DebouncedBuilder<S>
where
    S: Stream + Unpin,
{
    /// Returns a new builder for [`Debounced`].
    pub fn new() -> Self {
        Self {
            delay: Duration::from_secs(1),
            condition: None,
            max_waiting: None,
        }
    }
    /// Sets the delay for [`Debounced`].
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
    /// Sets the condition for [`Debounced`].
    pub fn conditional(mut self, conditional: fn(&S::Item) -> bool) -> Self {
        self.condition = Some(conditional);
        self
    }
    /// unimplemented
    #[allow(unused_variables, dead_code, unused_mut)]
    pub(crate) fn max_waiting(mut self, max_waiting: Duration) -> Self {
        unimplemented!();
        #[allow(unreachable_code)]
        {
            self.max_waiting = Some(max_waiting);
            self
        }
    }
    /// Builds the [`Debounced`] stream.
    pub fn build(self, s: S) -> Debounced<S> {
        Debounced::new(s, self.delay, self.condition, self.max_waiting)
    }
}
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use futures_channel::mpsc::channel;
    use futures_util::future::join;
    use futures_util::{SinkExt, StreamExt};
    use tokio::time::sleep;

    use crate::stream::DebouncedBuilder;

    use super::debounced;

    #[tokio::test]
    async fn test_debounce() {
        let start = Instant::now();
        let (mut sender, receiver) = futures_channel::mpsc::channel(1024);
        let mut debounced = debounced(receiver, Duration::from_secs(1));
        let _ = sender.send(21).await;
        let _ = sender.send(42).await;
        assert_eq!(debounced.next().await, Some(42));
        assert_eq!(start.elapsed().as_secs(), 1);
        std::mem::drop(sender);
        assert_eq!(debounced.next().await, None);
    }

    #[tokio::test]
    async fn test_debounce_if() {
        let start = Instant::now();
        let (mut sender, receiver) = futures_channel::mpsc::channel(1024);
        let mut debounced = DebouncedBuilder::new()
            .delay(Duration::from_secs(1))
            .conditional(|x| x < &30)
            .build(receiver);
        let _ = sender.send(21).await;
        let _ = sender.send(33).await;
        let _ = sender.send(24).await;
        let _ = sender.send(42).await;
        assert_eq!(debounced.next().await, Some(21));
        assert_eq!(debounced.next().await, Some(24));
        assert_eq!(debounced.next().await, Some(42));
        assert_eq!(start.elapsed().as_secs(), 1);
        std::mem::drop(sender);
        assert_eq!(debounced.next().await, None);
    }

    #[tokio::test]
    #[should_panic(expected = "not implemented")]
    async fn test_debounce_max_waiting() {
        let (mut sender, receiver) = futures_channel::mpsc::channel(1024);
        let mut debounced = DebouncedBuilder::new()
            .delay(Duration::from_secs(2))
            .max_waiting(Duration::from_secs(3))
            .build(receiver);
        let send_task = tokio::spawn(async move {
            println!("send task");
            let _ = sender.send(41).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = sender.send(33).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = sender.send(99).await;
            tokio::time::sleep(Duration::from_micros(1500)).await;
            let _ = sender.send(42).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let recv_task = tokio::spawn(async move {
            println!("recv task");
            assert_eq!(debounced.next().await, Some(99));
            assert_eq!(debounced.next().await, Some(42));
            assert_eq!(debounced.next().await, None);
        });

        join(send_task, recv_task).await.0.unwrap();
    }

    #[tokio::test]
    async fn test_debounce_order() {
        #[derive(Debug, PartialEq, Eq)]
        pub enum Message {
            Value(u64),
            SenderEnded,
            ReceiverEnded,
        }

        let (mut sender, receiver) = channel(1024);
        let mut receiver = debounced(receiver, Duration::from_millis(100));
        let messages = Arc::new(Mutex::new(vec![]));

        join(
            {
                let messages = messages.clone();
                async move {
                    for i in 0..10u64 {
                        let _ = sleep(Duration::from_millis(23 * i)).await;
                        let _ = sender.send(i).await;
                    }

                    messages.lock().unwrap().push(Message::SenderEnded);
                }
            },
            {
                let messages = messages.clone();

                async move {
                    while let Some(value) = receiver.next().await {
                        messages.lock().unwrap().push(Message::Value(value));
                    }

                    messages.lock().unwrap().push(Message::ReceiverEnded);
                }
            },
        )
        .await;

        assert_eq!(
            messages.lock().unwrap().as_slice(),
            &[
                Message::Value(4),
                Message::Value(5),
                Message::Value(6),
                Message::Value(7),
                Message::Value(8),
                Message::SenderEnded,
                Message::Value(9),
                Message::ReceiverEnded
            ]
        );
    }
}
