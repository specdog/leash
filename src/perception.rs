use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};

use crate::{
    types::{DetectionFrame, ImageObservation, VisionResult},
    worker::{WorkerInputFrame, WorkerInputPayload, WorkerOutputFrame, WorkerOutputPayload},
};

const DEFAULT_TIMEOUT_MS: u64 = 100;

pub trait PerceptionAdapter: Send + Sync + 'static {
    fn detect(&self, observation: ImageObservation) -> Result<VisionResult>;
}

#[derive(Clone)]
pub struct PerceptionRuntime {
    adapter: Arc<dyn PerceptionAdapter>,
    timeout: Duration,
}

impl Default for PerceptionRuntime {
    fn default() -> Self {
        Self::fake()
    }
}

impl PerceptionRuntime {
    pub fn fake() -> Self {
        Self::new(
            SimulatedPerceptionWorker,
            Duration::from_millis(DEFAULT_TIMEOUT_MS),
        )
    }

    pub fn new(adapter: impl PerceptionAdapter, timeout: Duration) -> Self {
        Self {
            adapter: Arc::new(adapter),
            timeout,
        }
    }

    pub fn observe(&self, observation: ImageObservation) -> VisionResult {
        let started = Instant::now();
        let observed_at_ms = observation.ts_ms;
        let source = observation.source.clone();
        let provider_source = source.clone();
        let adapter = Arc::clone(&self.adapter);
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| adapter.detect(observation)));
            let result = match result {
                Ok(result) => result,
                Err(_) => Ok(error_result(
                    observed_at_ms,
                    &provider_source,
                    "panic",
                    "perception provider panicked",
                    0,
                )),
            };
            let _ = tx.send(result);
        });

        match rx.recv_timeout(self.timeout) {
            Ok(Ok(mut result)) => {
                result.duration_ms = started.elapsed().as_millis();
                result
            }
            Ok(Err(err)) => error_result(
                observed_at_ms,
                &source,
                "error",
                &err.to_string(),
                started.elapsed().as_millis(),
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => error_result(
                observed_at_ms,
                &source,
                "timeout",
                "perception provider timed out",
                started.elapsed().as_millis(),
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => error_result(
                observed_at_ms,
                &source,
                "error",
                "perception provider disconnected",
                started.elapsed().as_millis(),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FakePerceptionAdapter;

impl PerceptionAdapter for FakePerceptionAdapter {
    fn detect(&self, observation: ImageObservation) -> Result<VisionResult> {
        let label = if observation.source == "replay" {
            "replay-fixture"
        } else {
            "sim-fixture"
        };
        Ok(VisionResult {
            ok: true,
            status: "ok".to_string(),
            source: "fake-perception".to_string(),
            observed_at_ms: observation.ts_ms,
            duration_ms: 0,
            detections: vec![DetectionFrame {
                ts_ms: observation.ts_ms,
                frame_id: observation.frame_id,
                id: format!("fake-{}-{}", observation.source, observation.ts_ms),
                label: label.to_string(),
                confidence: 0.82,
                x_m: 0.25,
                y_m: 0.0,
                width_m: 0.18,
                height_m: 0.12,
            }],
            error: None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SimulatedPerceptionWorker;

impl SimulatedPerceptionWorker {
    pub fn process(&self, input: WorkerInputFrame) -> Result<WorkerOutputFrame> {
        input.validate()?;
        let WorkerInputPayload::Perception { observation } = input.payload.clone() else {
            bail!("simulated perception worker requires a perception input frame");
        };
        let result = FakePerceptionAdapter.detect(observation)?;
        Ok(WorkerOutputFrame::vision(&input, result))
    }
}

impl PerceptionAdapter for SimulatedPerceptionWorker {
    fn detect(&self, observation: ImageObservation) -> Result<VisionResult> {
        let sequence = u64::try_from(observation.ts_ms).unwrap_or(u64::MAX);
        let output = self.process(WorkerInputFrame::perception(
            "simulated-perception",
            sequence,
            observation,
        ))?;
        match output.payload {
            WorkerOutputPayload::Vision { result } => Ok(result),
            WorkerOutputPayload::Error { message, .. } => bail!(message),
            WorkerOutputPayload::MotionEvents { .. } => {
                bail!("simulated perception worker returned motion events")
            }
        }
    }
}

fn error_result(
    observed_at_ms: u128,
    source: &str,
    status: &str,
    error: &str,
    duration_ms: u128,
) -> VisionResult {
    VisionResult {
        ok: false,
        status: status.to_string(),
        source: source.to_string(),
        observed_at_ms,
        duration_ms,
        detections: Vec::new(),
        error: Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;

    #[test]
    fn fake_detector_returns_deterministic_detection() {
        let runtime = PerceptionRuntime::fake();
        let first = runtime.observe(observation("sim", 42));
        let second = runtime.observe(observation("sim", 42));

        assert!(first.ok);
        assert_eq!(first.detections, second.detections);
        assert_eq!(first.detections[0].label, "sim-fixture");
    }

    #[test]
    fn simulated_worker_fixture_exercises_input_and_output_frames() {
        let input =
            WorkerInputFrame::perception("simulated-perception", 42, observation("sim", 42));
        let output = SimulatedPerceptionWorker.process(input).unwrap();

        assert_eq!(output.schema_version, crate::worker::WORKER_FRAME_VERSION);
        assert_eq!(output.sequence, 42);
        let WorkerOutputPayload::Vision { result } = output.payload else {
            panic!("simulated worker did not return a vision result");
        };
        assert_eq!(result.source, "fake-perception");
        assert_eq!(result.detections[0].label, "sim-fixture");
    }

    #[test]
    fn provider_error_is_returned_as_non_fatal_result() {
        let runtime = PerceptionRuntime::new(ErrorAdapter, Duration::from_millis(50));
        let result = runtime.observe(observation("sim", 7));

        assert!(!result.ok);
        assert_eq!(result.status, "error");
        assert!(result.error.unwrap().contains("provider unavailable"));
    }

    #[test]
    fn provider_panic_is_returned_as_non_fatal_result() {
        let runtime = PerceptionRuntime::new(PanicAdapter, Duration::from_millis(50));
        let result = runtime.observe(observation("sim", 7));

        assert!(!result.ok);
        assert_eq!(result.status, "panic");
    }

    #[test]
    fn provider_timeout_is_returned_as_non_fatal_result() {
        let runtime = PerceptionRuntime::new(SlowAdapter, Duration::from_millis(5));
        let result = runtime.observe(observation("sim", 7));

        assert!(!result.ok);
        assert_eq!(result.status, "timeout");
    }

    fn observation(source: &str, ts_ms: u128) -> ImageObservation {
        ImageObservation {
            ts_ms,
            frame_id: "camera".to_string(),
            source: source.to_string(),
            width_px: 640,
            height_px: 480,
            content_type: "image/simulated".to_string(),
            byte_len: 0,
            sha256: None,
        }
    }

    struct ErrorAdapter;

    impl PerceptionAdapter for ErrorAdapter {
        fn detect(&self, _observation: ImageObservation) -> Result<VisionResult> {
            bail!("provider unavailable")
        }
    }

    struct PanicAdapter;

    impl PerceptionAdapter for PanicAdapter {
        fn detect(&self, _observation: ImageObservation) -> Result<VisionResult> {
            panic!("provider panic")
        }
    }

    struct SlowAdapter;

    impl PerceptionAdapter for SlowAdapter {
        fn detect(&self, observation: ImageObservation) -> Result<VisionResult> {
            thread::sleep(Duration::from_millis(50));
            FakePerceptionAdapter.detect(observation)
        }
    }
}
