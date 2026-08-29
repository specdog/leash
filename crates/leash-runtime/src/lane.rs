use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowPolicy {
    RejectNewest,
    DropOldest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneCreateError;

impl fmt::Display for LaneCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded lane capacity must be non-zero")
    }
}

impl std::error::Error for LaneCreateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneSnapshot {
    pub capacity: usize,
    pub depth: usize,
    pub high_watermark: usize,
    pub sent: u64,
    pub received: u64,
    pub rejected: u64,
    pub dropped: u64,
    pub closed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome<T> {
    Enqueued,
    ReplacedOldest(T),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendError<T> {
    Closed(T),
    Full(T),
}

#[derive(Debug)]
struct LaneState<T> {
    queue: VecDeque<T>,
    high_watermark: usize,
    sent: u64,
    received: u64,
    rejected: u64,
    dropped: u64,
}

#[derive(Debug)]
struct LaneShared<T> {
    capacity: usize,
    policy: OverflowPolicy,
    closed: AtomicBool,
    state: Mutex<LaneState<T>>,
}

#[derive(Debug)]
pub struct BoundedSender<T> {
    shared: Arc<LaneShared<T>>,
}

impl<T> Clone for BoundedSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

#[derive(Debug)]
pub struct BoundedReceiver<T> {
    shared: Arc<LaneShared<T>>,
}

pub fn bounded_lane<T>(
    capacity: usize,
    policy: OverflowPolicy,
) -> Result<(BoundedSender<T>, BoundedReceiver<T>), LaneCreateError> {
    if capacity == 0 {
        return Err(LaneCreateError);
    }
    let shared = Arc::new(LaneShared {
        capacity,
        policy,
        closed: AtomicBool::new(false),
        state: Mutex::new(LaneState {
            queue: VecDeque::with_capacity(capacity),
            high_watermark: 0,
            sent: 0,
            received: 0,
            rejected: 0,
            dropped: 0,
        }),
    });
    Ok((
        BoundedSender {
            shared: Arc::clone(&shared),
        },
        BoundedReceiver { shared },
    ))
}

impl<T> BoundedSender<T> {
    pub fn try_send(&self, value: T) -> Result<SendOutcome<T>, SendError<T>> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SendError::Closed(value));
        }
        let mut state = lock(&self.shared.state);
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SendError::Closed(value));
        }
        let outcome = if state.queue.len() == self.shared.capacity {
            match self.shared.policy {
                OverflowPolicy::RejectNewest => {
                    state.rejected = state.rejected.saturating_add(1);
                    return Err(SendError::Full(value));
                }
                OverflowPolicy::DropOldest => {
                    let dropped = state
                        .queue
                        .pop_front()
                        .expect("a full bounded queue contains an item");
                    state.dropped = state.dropped.saturating_add(1);
                    state.queue.push_back(value);
                    SendOutcome::ReplacedOldest(dropped)
                }
            }
        } else {
            state.queue.push_back(value);
            SendOutcome::Enqueued
        };
        state.sent = state.sent.saturating_add(1);
        state.high_watermark = state.high_watermark.max(state.queue.len());
        Ok(outcome)
    }

    pub fn snapshot(&self) -> LaneSnapshot {
        snapshot(&self.shared)
    }
}

impl<T> BoundedReceiver<T> {
    pub fn try_recv(&mut self) -> Option<T> {
        let mut state = lock(&self.shared.state);
        let value = state.queue.pop_front();
        if value.is_some() {
            state.received = state.received.saturating_add(1);
        }
        value
    }

    pub fn snapshot(&self) -> LaneSnapshot {
        snapshot(&self.shared)
    }
}

impl<T> Drop for BoundedReceiver<T> {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
    }
}

fn snapshot<T>(shared: &LaneShared<T>) -> LaneSnapshot {
    let state = lock(&shared.state);
    LaneSnapshot {
        capacity: shared.capacity,
        depth: state.queue.len(),
        high_watermark: state.high_watermark,
        sent: state.sent,
        received: state.received,
        rejected: state.rejected,
        dropped: state.dropped,
        closed: shared.closed.load(Ordering::Acquire),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_newest_is_bounded_and_measured() {
        let (sender, mut receiver) = bounded_lane(2, OverflowPolicy::RejectNewest).unwrap();
        assert_eq!(sender.try_send(1), Ok(SendOutcome::Enqueued));
        assert_eq!(sender.try_send(2), Ok(SendOutcome::Enqueued));
        assert_eq!(sender.try_send(3), Err(SendError::Full(3)));
        assert_eq!(receiver.try_recv(), Some(1));
        assert_eq!(receiver.try_recv(), Some(2));
        assert_eq!(receiver.try_recv(), None);
        assert_eq!(
            sender.snapshot(),
            LaneSnapshot {
                capacity: 2,
                depth: 0,
                high_watermark: 2,
                sent: 2,
                received: 2,
                rejected: 1,
                dropped: 0,
                closed: false,
            }
        );
    }

    #[test]
    fn drop_oldest_preserves_the_newest_bounded_work() {
        let (sender, mut receiver) = bounded_lane(2, OverflowPolicy::DropOldest).unwrap();
        sender.try_send(1).unwrap();
        sender.try_send(2).unwrap();
        assert_eq!(sender.try_send(3), Ok(SendOutcome::ReplacedOldest(1)));
        assert_eq!(receiver.try_recv(), Some(2));
        assert_eq!(receiver.try_recv(), Some(3));
        let snapshot = receiver.snapshot();
        assert_eq!(snapshot.sent, 3);
        assert_eq!(snapshot.dropped, 1);
        assert_eq!(snapshot.high_watermark, 2);
    }

    #[test]
    fn receiver_drop_closes_every_sender() {
        let (sender, receiver) = bounded_lane(1, OverflowPolicy::RejectNewest).unwrap();
        let second = sender.clone();
        drop(receiver);
        assert_eq!(sender.try_send(1), Err(SendError::Closed(1)));
        assert_eq!(second.try_send(2), Err(SendError::Closed(2)));
        assert!(sender.snapshot().closed);
    }
}
