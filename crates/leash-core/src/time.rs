use core::num::NonZeroU64;

use crate::DomainError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonotonicNanos(u64);

impl MonotonicNanos {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_add(self, duration: DurationNanos) -> Result<Self, DomainError> {
        self.0
            .checked_add(duration.0)
            .map(Self)
            .ok_or(DomainError::Overflow("monotonic timestamp"))
    }

    pub fn duration_since(self, earlier: Self) -> Result<DurationNanos, DomainError> {
        self.0
            .checked_sub(earlier.0)
            .map(DurationNanos)
            .ok_or(DomainError::TimeReversed)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationNanos(u64);

impl DurationNanos {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn from_millis(value: u64) -> Result<Self, DomainError> {
        value
            .checked_mul(1_000_000)
            .map(Self)
            .ok_or(DomainError::Overflow("duration"))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sequence(NonZeroU64);

impl Sequence {
    pub fn new(value: u64) -> Result<Self, DomainError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(DomainError::Zero("sequence"))
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub fn next(self) -> Result<Self, DomainError> {
        self.get()
            .checked_add(1)
            .ok_or(DomainError::Overflow("sequence"))
            .and_then(Self::new)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProducerEpoch(NonZeroU64);

impl ProducerEpoch {
    pub fn new(value: u64) -> Result<Self, DomainError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(DomainError::Zero("producer epoch"))
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped<T> {
    pub at: MonotonicNanos,
    pub sequence: Sequence,
    pub value: T,
}

impl<T> Stamped<T> {
    pub const fn new(at: MonotonicNanos, sequence: Sequence, value: T) -> Self {
        Self {
            at,
            sequence,
            value,
        }
    }

    pub fn map<U>(self, transform: impl FnOnce(T) -> U) -> Stamped<U> {
        Stamped {
            at: self.at,
            sequence: self.sequence,
            value: transform(self.value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_time_rejects_reversal_and_overflow() {
        let earlier = MonotonicNanos::new(10);
        let later = MonotonicNanos::new(25);
        assert_eq!(later.duration_since(earlier).unwrap().get(), 15);
        assert_eq!(
            earlier.duration_since(later),
            Err(DomainError::TimeReversed)
        );
        assert_eq!(
            MonotonicNanos::new(u64::MAX).checked_add(DurationNanos::new(1)),
            Err(DomainError::Overflow("monotonic timestamp"))
        );
        assert_eq!(
            DurationNanos::from_millis(u64::MAX),
            Err(DomainError::Overflow("duration"))
        );
    }

    #[test]
    fn identifiers_are_non_zero_and_checked() {
        assert_eq!(Sequence::new(0), Err(DomainError::Zero("sequence")));
        assert_eq!(
            ProducerEpoch::new(0),
            Err(DomainError::Zero("producer epoch"))
        );
        assert_eq!(Sequence::new(1).unwrap().next().unwrap().get(), 2);
    }
}
