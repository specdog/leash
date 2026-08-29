use std::sync::{Arc, Mutex, MutexGuard};

use leash_core::{Sequence, Stamped};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatestSnapshot {
    pub occupied: bool,
    pub last_sequence: Option<Sequence>,
    pub published: u64,
    pub replaced: u64,
    pub taken: u64,
    pub rejected_out_of_order: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublishError<T> {
    SequenceNotIncreasing(Stamped<T>),
}

#[derive(Debug)]
struct LatestState<T> {
    value: Option<Stamped<T>>,
    last_sequence: Option<Sequence>,
    published: u64,
    replaced: u64,
    taken: u64,
    rejected_out_of_order: u64,
}

#[derive(Debug)]
pub struct LatestPublisher<T> {
    state: Arc<Mutex<LatestState<T>>>,
}

impl<T> Clone for LatestPublisher<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

#[derive(Debug)]
pub struct LatestReader<T> {
    state: Arc<Mutex<LatestState<T>>>,
}

pub fn latest_slot<T>() -> (LatestPublisher<T>, LatestReader<T>) {
    let state = Arc::new(Mutex::new(LatestState {
        value: None,
        last_sequence: None,
        published: 0,
        replaced: 0,
        taken: 0,
        rejected_out_of_order: 0,
    }));
    (
        LatestPublisher {
            state: Arc::clone(&state),
        },
        LatestReader { state },
    )
}

impl<T> LatestPublisher<T> {
    pub fn publish(&self, value: Stamped<T>) -> Result<Option<Stamped<T>>, PublishError<T>> {
        let mut state = lock(&self.state);
        if state
            .last_sequence
            .is_some_and(|sequence| value.sequence <= sequence)
        {
            state.rejected_out_of_order = state.rejected_out_of_order.saturating_add(1);
            return Err(PublishError::SequenceNotIncreasing(value));
        }
        state.last_sequence = Some(value.sequence);
        state.published = state.published.saturating_add(1);
        let replaced = state.value.replace(value);
        if replaced.is_some() {
            state.replaced = state.replaced.saturating_add(1);
        }
        Ok(replaced)
    }

    pub fn snapshot(&self) -> LatestSnapshot {
        snapshot(&self.state)
    }
}

impl<T> LatestReader<T> {
    pub fn take(&mut self) -> Option<Stamped<T>> {
        let mut state = lock(&self.state);
        let value = state.value.take();
        if value.is_some() {
            state.taken = state.taken.saturating_add(1);
        }
        value
    }

    pub fn snapshot(&self) -> LatestSnapshot {
        snapshot(&self.state)
    }
}

fn snapshot<T>(state: &Mutex<LatestState<T>>) -> LatestSnapshot {
    let state = lock(state);
    LatestSnapshot {
        occupied: state.value.is_some(),
        last_sequence: state.last_sequence,
        published: state.published,
        replaced: state.replaced,
        taken: state.taken,
        rejected_out_of_order: state.rejected_out_of_order,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use leash_core::MonotonicNanos;

    use super::*;

    fn sample(sequence: u64, value: u64) -> Stamped<u64> {
        Stamped::new(
            MonotonicNanos::new(sequence),
            Sequence::new(sequence).unwrap(),
            value,
        )
    }

    #[test]
    fn latest_value_replaces_stale_work_and_rejects_reordering() {
        let (publisher, mut reader) = latest_slot();
        assert_eq!(publisher.publish(sample(1, 10)), Ok(None));
        assert_eq!(publisher.publish(sample(2, 20)).unwrap().unwrap().value, 10);
        assert_eq!(
            publisher.publish(sample(2, 30)),
            Err(PublishError::SequenceNotIncreasing(sample(2, 30)))
        );
        assert_eq!(reader.take().unwrap().value, 20);
        assert!(reader.take().is_none());
        assert_eq!(
            reader.snapshot(),
            LatestSnapshot {
                occupied: false,
                last_sequence: Some(Sequence::new(2).unwrap()),
                published: 2,
                replaced: 1,
                taken: 1,
                rejected_out_of_order: 1,
            }
        );
    }
}
