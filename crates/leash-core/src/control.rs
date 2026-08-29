use crate::{Authorized, CommandId, MonotonicNanos, Sequence, Stamped};

pub trait Clock {
    fn now(&mut self) -> MonotonicNanos;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick<I> {
    pub at: MonotonicNanos,
    pub sequence: Sequence,
    pub input: I,
}

impl<I> Tick<I> {
    pub const fn new(at: MonotonicNanos, sequence: Sequence, input: I) -> Self {
        Self {
            at,
            sequence,
            input,
        }
    }
}

pub const DEFAULT_EFFECT_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effects<O, const N: usize = DEFAULT_EFFECT_CAPACITY> {
    pub at: MonotonicNanos,
    outputs: [Option<O>; N],
    len: usize,
}

impl<O, const N: usize> Effects<O, N> {
    pub fn none(at: MonotonicNanos) -> Self {
        Self {
            at,
            outputs: std::array::from_fn(|_| None),
            len: 0,
        }
    }

    pub fn one(at: MonotonicNanos, output: O) -> Self {
        let mut effects = Self::none(at);
        effects
            .push(output)
            .unwrap_or_else(|_| unreachable!("one effect requires non-zero capacity"));
        effects
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, output: O) -> Result<(), O> {
        if self.len == N {
            return Err(output);
        }
        self.outputs[self.len] = Some(output);
        self.len += 1;
        Ok(())
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &O> {
        self.outputs[..self.len]
            .iter()
            .map(|output| output.as_ref().expect("occupied effect slot"))
    }
}

pub trait Controller {
    type Input;
    type Output;
    type Error;

    fn step(&mut self, tick: Tick<Self::Input>) -> Result<Effects<Self::Output>, Self::Error>;
}

pub trait SensorSource {
    type Sample;
    type Error;

    fn poll(&mut self, now: MonotonicNanos) -> Result<Option<Stamped<Self::Sample>>, Self::Error>;
}

pub trait ActuatorSink {
    type Command;
    type Acknowledgement;
    type Error;

    fn apply(
        &mut self,
        command: Authorized<Self::Command>,
    ) -> Result<Self::Acknowledgement, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeRequest<J> {
    pub id: CommandId,
    pub submitted_at: MonotonicNanos,
    pub deadline: MonotonicNanos,
    pub job: J,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeCompletion<R> {
    pub id: CommandId,
    pub completed_at: MonotonicNanos,
    pub result: R,
}

pub trait ComputeBackend {
    type Job;
    type Result;
    type Error;

    fn submit(&mut self, request: ComputeRequest<Self::Job>) -> Result<(), Self::Error>;

    fn try_complete(&mut self) -> Result<Option<ComputeCompletion<Self::Result>>, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DomainError;

    #[derive(Debug, Default)]
    struct Counter(u64);

    impl Controller for Counter {
        type Input = u64;
        type Output = u64;
        type Error = DomainError;

        fn step(&mut self, tick: Tick<Self::Input>) -> Result<Effects<Self::Output>, Self::Error> {
            self.0 = self
                .0
                .checked_add(tick.input)
                .ok_or(DomainError::Overflow("counter"))?;
            Ok(Effects::one(tick.at, self.0))
        }
    }

    #[test]
    fn controller_transition_is_owned_and_clock_explicit() {
        let mut counter = Counter::default();
        let tick = Tick::new(MonotonicNanos::new(50), Sequence::new(1).unwrap(), 4);
        let effects = counter.step(tick).unwrap();
        assert_eq!(effects.at, MonotonicNanos::new(50));
        assert_eq!(effects.iter().copied().collect::<Vec<_>>(), [4]);
    }
}
