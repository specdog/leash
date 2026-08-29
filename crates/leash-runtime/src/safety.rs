use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyKind {
    Stop,
    EStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetySignal {
    pub kind: SafetyKind,
    pub first_sequence: u64,
    pub through_sequence: u64,
    pub coalesced: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyRequestError {
    Closed,
    SequenceExhausted,
}

impl fmt::Display for SafetyRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("safety mailbox is closed"),
            Self::SequenceExhausted => formatter.write_str("safety request sequence exhausted"),
        }
    }
}

impl std::error::Error for SafetyRequestError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyReceiveError {
    Closed,
}

#[derive(Debug)]
struct SafetyShared {
    stop: AtomicU64,
    estop: AtomicU64,
    closed: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct SafetySender {
    shared: Arc<SafetyShared>,
}

#[derive(Debug)]
pub struct SafetyReceiver {
    shared: Arc<SafetyShared>,
    seen_stop: u64,
    seen_estop: u64,
}

pub fn safety_mailbox() -> (SafetySender, SafetyReceiver) {
    let shared = Arc::new(SafetyShared {
        stop: AtomicU64::new(0),
        estop: AtomicU64::new(0),
        closed: AtomicBool::new(false),
    });
    (
        SafetySender {
            shared: Arc::clone(&shared),
        },
        SafetyReceiver {
            shared,
            seen_stop: 0,
            seen_estop: 0,
        },
    )
}

impl SafetySender {
    pub fn request(&self, kind: SafetyKind) -> Result<u64, SafetyRequestError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SafetyRequestError::Closed);
        }
        let sequence = match kind {
            SafetyKind::Stop => increment(&self.shared.stop),
            SafetyKind::EStop => increment(&self.shared.estop),
        }?;
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SafetyRequestError::Closed);
        }
        Ok(sequence)
    }

    pub fn stop(&self) -> Result<u64, SafetyRequestError> {
        self.request(SafetyKind::Stop)
    }

    pub fn estop(&self) -> Result<u64, SafetyRequestError> {
        self.request(SafetyKind::EStop)
    }
}

impl SafetyReceiver {
    pub fn try_recv(&mut self) -> Result<Option<SafetySignal>, SafetyReceiveError> {
        let estop = self.shared.estop.load(Ordering::Acquire);
        if estop > self.seen_estop {
            let signal = signal(SafetyKind::EStop, self.seen_estop, estop);
            self.seen_estop = estop;
            return Ok(Some(signal));
        }
        let stop = self.shared.stop.load(Ordering::Acquire);
        if stop > self.seen_stop {
            let signal = signal(SafetyKind::Stop, self.seen_stop, stop);
            self.seen_stop = stop;
            return Ok(Some(signal));
        }
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SafetyReceiveError::Closed);
        }
        Ok(None)
    }
}

impl Drop for SafetyReceiver {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
    }
}

fn increment(counter: &AtomicU64) -> Result<u64, SafetyRequestError> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(1)
        })
        .map(|previous| previous + 1)
        .map_err(|_| SafetyRequestError::SequenceExhausted)
}

fn signal(kind: SafetyKind, seen: u64, requested: u64) -> SafetySignal {
    SafetySignal {
        kind,
        first_sequence: seen + 1,
        through_sequence: requested,
        coalesced: requested - seen - 1,
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn estop_preempts_stop_and_request_counts_are_preserved() {
        let (sender, mut receiver) = safety_mailbox();
        sender.stop().unwrap();
        sender.stop().unwrap();
        sender.estop().unwrap();
        sender.estop().unwrap();
        sender.estop().unwrap();

        assert_eq!(
            receiver.try_recv(),
            Ok(Some(SafetySignal {
                kind: SafetyKind::EStop,
                first_sequence: 1,
                through_sequence: 3,
                coalesced: 2,
            }))
        );
        assert_eq!(
            receiver.try_recv(),
            Ok(Some(SafetySignal {
                kind: SafetyKind::Stop,
                first_sequence: 1,
                through_sequence: 2,
                coalesced: 1,
            }))
        );
        assert_eq!(receiver.try_recv(), Ok(None));
    }

    #[test]
    fn concurrent_stop_requests_are_counted_without_a_queue() {
        let (sender, mut receiver) = safety_mailbox();
        let workers = (0..4)
            .map(|_| {
                let sender = sender.clone();
                thread::spawn(move || {
                    for _ in 0..1_000 {
                        sender.stop().unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }

        let signal = receiver.try_recv().unwrap().unwrap();
        assert_eq!(signal.kind, SafetyKind::Stop);
        assert_eq!(signal.first_sequence, 1);
        assert_eq!(signal.through_sequence, 4_000);
        assert_eq!(signal.coalesced, 3_999);
    }

    #[test]
    fn receiver_drop_closes_safety_requests() {
        let (sender, receiver) = safety_mailbox();
        drop(receiver);
        assert_eq!(sender.stop(), Err(SafetyRequestError::Closed));
    }
}
