//! Host timing statistics, independent of renderer internals and game assets.

use std::time::Duration;

/// Durations for one frame; GPU wait is populated only by the offscreen
/// benchmark, where every frame waits for completion.
#[derive(Clone, Copy, Default)]
pub(crate) struct FrameSample {
    pub frame: Duration,
    pub simulation: Duration,
    pub acquire: Duration,
    pub render: Duration,
    pub ui_present: Duration,
    pub gpu_wait: Duration,
}

#[derive(Default)]
pub(crate) struct FrameProfile {
    frames: Vec<Duration>,
    totals: FrameSample,
}

pub(crate) struct FrameSummary {
    pub frames: usize,
    pub fps: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub simulation_ms: f64,
    pub acquire_ms: f64,
    pub render_ms: f64,
    pub ui_present_ms: f64,
    pub gpu_wait_ms: f64,
}

impl FrameProfile {
    pub(crate) fn record(&mut self, sample: FrameSample) {
        self.frames.push(sample.frame);
        self.totals.simulation += sample.simulation;
        self.totals.acquire += sample.acquire;
        self.totals.render += sample.render;
        self.totals.ui_present += sample.ui_present;
        self.totals.gpu_wait += sample.gpu_wait;
    }

    /// Nearest-rank percentiles over completed samples. Reset while keeping
    /// capacity so periodic window profiling does not keep growing memory.
    pub(crate) fn finish(&mut self, elapsed: Duration) -> Option<FrameSummary> {
        if self.frames.is_empty() || elapsed.is_zero() {
            return None;
        }
        self.frames.sort_unstable();
        let count = self.frames.len();
        #[allow(clippy::cast_precision_loss)]
        let divisor = count as f64;
        let mean_ms = |duration: Duration| duration.as_secs_f64() * 1000.0 / divisor;
        let summary = FrameSummary {
            frames: count,
            fps: divisor / elapsed.as_secs_f64(),
            median_ms: self.frames[count.div_ceil(2) - 1].as_secs_f64() * 1000.0,
            p95_ms: self.frames[(count * 95).div_ceil(100) - 1].as_secs_f64() * 1000.0,
            simulation_ms: mean_ms(self.totals.simulation),
            acquire_ms: mean_ms(self.totals.acquire),
            render_ms: mean_ms(self.totals.render),
            ui_present_ms: mean_ms(self.totals.ui_present),
            gpu_wait_ms: mean_ms(self.totals.gpu_wait),
        };
        self.frames.clear();
        self.totals = FrameSample::default();
        Some(summary)
    }
}

impl FrameSummary {
    pub(crate) fn log(&self, mode: &'static str) {
        tracing::info!(
            mode,
            frames = self.frames,
            fps = format_args!("{:.1}", self.fps),
            median_ms = format_args!("{:.3}", self.median_ms),
            p95_ms = format_args!("{:.3}", self.p95_ms),
            simulation_ms = format_args!("{:.3}", self.simulation_ms),
            acquire_ms = format_args!("{:.3}", self.acquire_ms),
            render_submit_ms = format_args!("{:.3}", self.render_ms),
            ui_present_ms = format_args!("{:.3}", self.ui_present_ms),
            gpu_wait_ms = format_args!("{:.3}", self.gpu_wait_ms),
            "frame profile"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameProfile, FrameSample};
    use std::time::Duration;

    #[test]
    fn summary_uses_elapsed_time_and_nearest_rank_percentiles_and_resets() {
        let mut profile = FrameProfile::default();
        for millis in (1..=20).rev() {
            profile.record(FrameSample {
                frame: Duration::from_millis(millis),
                simulation: Duration::from_millis(2),
                render: Duration::from_millis(3),
                ..FrameSample::default()
            });
        }
        let summary = profile.finish(Duration::from_secs(2)).unwrap();
        assert_eq!(summary.frames, 20);
        for (actual, expected) in [
            (summary.fps, 10.0),
            (summary.median_ms, 10.0),
            (summary.p95_ms, 19.0),
            (summary.simulation_ms, 2.0),
            (summary.render_ms, 3.0),
        ] {
            assert!((actual - expected).abs() < 1e-9);
        }
        assert!(profile.finish(Duration::from_secs(1)).is_none());
    }

    #[test]
    fn zero_elapsed_retains_samples_until_a_valid_summary() {
        let mut profile = FrameProfile::default();
        profile.record(FrameSample {
            frame: Duration::from_millis(7),
            ..FrameSample::default()
        });
        assert!(profile.finish(Duration::ZERO).is_none());
        let summary = profile.finish(Duration::from_millis(7)).unwrap();
        assert_eq!(summary.frames, 1);
        assert!((summary.median_ms - 7.0).abs() < 1e-9);
        assert!((summary.p95_ms - 7.0).abs() < 1e-9);
    }
}
