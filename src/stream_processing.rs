use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestValue<T> {
    value: Option<T>,
    dropped: u64,
}

impl<T> Default for LatestValue<T> {
    fn default() -> Self {
        Self {
            value: None,
            dropped: 0,
        }
    }
}

impl<T> LatestValue<T> {
    pub fn push(&mut self, value: T) {
        if self.value.replace(value).is_some() {
            self.dropped += 1;
        }
    }

    pub fn take(&mut self) -> Option<T> {
        self.value.take()
    }

    pub fn latest(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[derive(Debug, Clone)]
pub struct RateLimiter<K> {
    min_interval: Duration,
    last_emit: HashMap<K, Instant>,
}

impl<K> RateLimiter<K>
where
    K: Eq + Hash + Clone,
{
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_emit: HashMap::new(),
        }
    }

    pub fn allow(&mut self, key: K, now: Instant) -> bool {
        let Some(last) = self.last_emit.get(&key).copied() else {
            self.last_emit.insert(key, now);
            return true;
        };
        if now.duration_since(last) < self.min_interval {
            return false;
        }
        self.last_emit.insert(key, now);
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum FrameQuality {
    Drop,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct QualityDecision {
    pub accept: bool,
    pub quality: FrameQuality,
    pub reason: Option<String>,
}

impl QualityDecision {
    pub fn accept(quality: FrameQuality) -> Self {
        Self {
            accept: quality != FrameQuality::Drop,
            quality,
            reason: None,
        }
    }

    pub fn reject(reason: impl Into<String>) -> Self {
        Self {
            accept: false,
            quality: FrameQuality::Drop,
            reason: Some(reason.into()),
        }
    }
}

pub trait QualityFilter<T> {
    fn evaluate(&self, frame: &T) -> QualityDecision;
}

impl<T, F> QualityFilter<T> for F
where
    F: Fn(&T) -> QualityDecision,
{
    fn evaluate(&self, frame: &T) -> QualityDecision {
        self(frame)
    }
}

pub fn select_best_frame<T>(
    frames: impl IntoIterator<Item = T>,
    filter: impl QualityFilter<T>,
) -> Option<(T, QualityDecision)> {
    frames
        .into_iter()
        .filter_map(|frame| {
            let decision = filter.evaluate(&frame);
            decision.accept.then_some((frame, decision))
        })
        .max_by_key(|(_, decision)| decision.quality)
}

pub trait Timestamped {
    fn ts_ms(&self) -> u128;
}

impl<T> Timestamped for &T
where
    T: Timestamped,
{
    fn ts_ms(&self) -> u128 {
        (*self).ts_ms()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampPair<A, B> {
    pub left: A,
    pub right: B,
    pub delta_ms: u128,
}

pub fn pair_by_timestamp<A, B>(
    left: impl IntoIterator<Item = A>,
    right: impl IntoIterator<Item = B>,
    tolerance_ms: u128,
) -> Vec<TimestampPair<A, B>>
where
    A: Timestamped,
    B: Timestamped,
{
    let mut right = right.into_iter().collect::<Vec<_>>();
    let mut pairs = Vec::new();

    for left_item in left {
        let Some((index, delta_ms)) = right
            .iter()
            .enumerate()
            .map(|(index, right_item)| (index, abs_delta(left_item.ts_ms(), right_item.ts_ms())))
            .filter(|(_, delta_ms)| *delta_ms <= tolerance_ms)
            .min_by_key(|(_, delta_ms)| *delta_ms)
        else {
            continue;
        };
        let right_item = right.remove(index);
        pairs.push(TimestampPair {
            left: left_item,
            right: right_item,
            delta_ms,
        });
    }

    pairs
}

fn abs_delta(left: u128, right: u128) -> u128 {
    left.abs_diff(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Sample {
        ts_ms: u128,
        score: u8,
        corrupt: bool,
    }

    impl Timestamped for Sample {
        fn ts_ms(&self) -> u128 {
            self.ts_ms
        }
    }

    #[test]
    fn latest_value_keeps_only_newest_frame() {
        let mut latest = LatestValue::default();

        latest.push("old");
        latest.push("newer");
        latest.push("newest");

        assert_eq!(latest.dropped(), 2);
        assert_eq!(latest.take(), Some("newest"));
        assert_eq!(latest.take(), None);
    }

    #[test]
    fn rate_limiter_blocks_until_interval_elapses() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(Duration::from_millis(100));

        assert!(limiter.allow("camera", start));
        assert!(!limiter.allow("camera", start + Duration::from_millis(99)));
        assert!(limiter.allow("camera", start + Duration::from_millis(100)));
        assert!(limiter.allow("lidar", start + Duration::from_millis(1)));
    }

    #[test]
    fn quality_filter_selects_best_accepted_frame() {
        let frames = vec![
            Sample {
                ts_ms: 1,
                score: 80,
                corrupt: true,
            },
            Sample {
                ts_ms: 2,
                score: 30,
                corrupt: false,
            },
            Sample {
                ts_ms: 3,
                score: 90,
                corrupt: false,
            },
        ];

        let (frame, decision) = select_best_frame(frames, |frame: &Sample| {
            if frame.corrupt {
                return QualityDecision::reject("corrupt");
            }
            QualityDecision::accept(if frame.score >= 75 {
                FrameQuality::High
            } else {
                FrameQuality::Low
            })
        })
        .unwrap();

        assert_eq!(frame.ts_ms, 3);
        assert_eq!(decision.quality, FrameQuality::High);
    }

    #[test]
    fn timestamp_pairing_honors_tolerance_and_consumes_matches() {
        let left = vec![
            Sample {
                ts_ms: 100,
                score: 1,
                corrupt: false,
            },
            Sample {
                ts_ms: 150,
                score: 2,
                corrupt: false,
            },
            Sample {
                ts_ms: 300,
                score: 3,
                corrupt: false,
            },
        ];
        let right = vec![
            Sample {
                ts_ms: 110,
                score: 10,
                corrupt: false,
            },
            Sample {
                ts_ms: 140,
                score: 20,
                corrupt: false,
            },
            Sample {
                ts_ms: 500,
                score: 30,
                corrupt: false,
            },
        ];

        let pairs = pair_by_timestamp(left, right, 15);

        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].left.ts_ms, 100);
        assert_eq!(pairs[0].right.ts_ms, 110);
        assert_eq!(pairs[0].delta_ms, 10);
        assert_eq!(pairs[1].left.ts_ms, 150);
        assert_eq!(pairs[1].right.ts_ms, 140);
        assert_eq!(pairs[1].delta_ms, 10);
    }
}
