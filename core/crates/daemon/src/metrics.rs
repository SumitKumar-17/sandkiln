//! Minimal Prometheus text-exposition-format metrics, hand-rolled rather
//! than pulling in the `prometheus` crate: the surface here is four
//! metrics behind a couple of atomics and a small histogram, and the
//! text format itself
//! (<https://prometheus.io/docs/instrumenting/exposition_formats/>) is a
//! few lines of string formatting per metric — not enough to justify a
//! new dependency and its own registry/collector abstraction.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub struct Metrics {
    sandboxes_created_total: AtomicU64,
    boot_duration_ms: Histogram,
    exec_latency_ms: Histogram,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            sandboxes_created_total: AtomicU64::new(0),
            // Real cold boots measure ~30ms (see ROADMAP.md's benchmarking
            // numbers) — bucketed comfortably below and above that so a
            // regression shows up as a bucket shift, not just a mean.
            boot_duration_ms: Histogram::new(&[10.0, 25.0, 50.0, 100.0, 250.0, 1000.0]),
            // Exec latency spans a much wider range: sub-millisecond on an
            // already-open vsock connection, hundreds of ms through the
            // full HTTP path against a freshly booted agent (see
            // ROADMAP.md's load-test numbers) — bucketed accordingly.
            exec_latency_ms: Histogram::new(&[1.0, 10.0, 50.0, 250.0, 1000.0]),
        }
    }

    pub fn record_sandbox_created(&self) {
        self.sandboxes_created_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_boot_duration_ms(&self, ms: f64) {
        self.boot_duration_ms.observe(ms);
    }

    pub fn record_exec_latency_ms(&self, ms: f64) {
        self.exec_latency_ms.observe(ms);
    }

    /// Renders the full exposition-format text. `sandboxes_active` is
    /// passed in rather than tracked as its own atomic — the sandbox map
    /// is already the source of truth for "how many are active", so this
    /// just reads its current length instead of keeping a second counter
    /// that could drift out of sync with it.
    pub fn render(&self, sandboxes_active: usize) -> String {
        let mut out = String::new();

        out.push_str("# HELP sandboxes_created_total Total number of sandboxes created since the daemon started.\n");
        out.push_str("# TYPE sandboxes_created_total counter\n");
        out.push_str(&format!("sandboxes_created_total {}\n", self.sandboxes_created_total.load(Ordering::Relaxed)));

        out.push_str("# HELP sandboxes_active Number of sandboxes currently tracked by the daemon.\n");
        out.push_str("# TYPE sandboxes_active gauge\n");
        out.push_str(&format!("sandboxes_active {sandboxes_active}\n"));

        out.push_str("# HELP boot_duration_ms Microvm boot duration in milliseconds.\n");
        out.push_str("# TYPE boot_duration_ms histogram\n");
        self.boot_duration_ms.render("boot_duration_ms", &mut out);

        out.push_str("# HELP exec_latency_ms Guest agent exec round-trip latency in milliseconds.\n");
        out.push_str("# TYPE exec_latency_ms histogram\n");
        self.exec_latency_ms.render("exec_latency_ms", &mut out);

        out
    }
}

struct Histogram {
    bounds: Vec<f64>,
    state: Mutex<HistogramState>,
}

struct HistogramState {
    /// `bucket_counts[i]` is the number of observations with
    /// `bounds[i-1] < v <= bounds[i]` (`bounds[-1]` treated as `-inf` for
    /// `i == 0`). An observation above the largest bound doesn't land in
    /// any finite bucket — only `sum`/`count` change for it — since it's
    /// covered by the implicit `le="+Inf"` line at render time, which is
    /// always just the total count.
    bucket_counts: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Histogram {
    fn new(bounds: &[f64]) -> Self {
        let bucket_counts = vec![0; bounds.len()];
        Self { bounds: bounds.to_vec(), state: Mutex::new(HistogramState { bucket_counts, sum: 0.0, count: 0 }) }
    }

    fn observe(&self, value: f64) {
        let mut state = self.state.lock().unwrap();
        if let Some(idx) = self.bounds.iter().position(|&bound| value <= bound) {
            state.bucket_counts[idx] += 1;
        }
        state.sum += value;
        state.count += 1;
    }

    fn render(&self, name: &str, out: &mut String) {
        let state = self.state.lock().unwrap();
        let mut cumulative = 0u64;
        for (bound, &bucket_count) in self.bounds.iter().zip(state.bucket_counts.iter()) {
            cumulative += bucket_count;
            out.push_str(&format!("{name}_bucket{{le=\"{}\"}} {cumulative}\n", fmt_bound(*bound)));
        }
        out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {}\n", state.count));
        out.push_str(&format!("{name}_sum {}\n", fmt_bound(state.sum)));
        out.push_str(&format!("{name}_count {}\n", state.count));
    }
}

/// Formats a bucket bound / sum the way Prometheus text format expects —
/// whole numbers without a trailing `.0`, since bucket boundaries here are
/// always round numbers and this keeps the label values matching what an
/// operator actually typed in a query (`le="50"`, not `le="50.5e1"` or
/// similar float-formatting surprises).
fn fmt_bound(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_metrics_render_all_zero() {
        let metrics = Metrics::new();
        assert_eq!(
            metrics.render(0),
            "# HELP sandboxes_created_total Total number of sandboxes created since the daemon started.\n\
             # TYPE sandboxes_created_total counter\n\
             sandboxes_created_total 0\n\
             # HELP sandboxes_active Number of sandboxes currently tracked by the daemon.\n\
             # TYPE sandboxes_active gauge\n\
             sandboxes_active 0\n\
             # HELP boot_duration_ms Microvm boot duration in milliseconds.\n\
             # TYPE boot_duration_ms histogram\n\
             boot_duration_ms_bucket{le=\"10\"} 0\n\
             boot_duration_ms_bucket{le=\"25\"} 0\n\
             boot_duration_ms_bucket{le=\"50\"} 0\n\
             boot_duration_ms_bucket{le=\"100\"} 0\n\
             boot_duration_ms_bucket{le=\"250\"} 0\n\
             boot_duration_ms_bucket{le=\"1000\"} 0\n\
             boot_duration_ms_bucket{le=\"+Inf\"} 0\n\
             boot_duration_ms_sum 0\n\
             boot_duration_ms_count 0\n\
             # HELP exec_latency_ms Guest agent exec round-trip latency in milliseconds.\n\
             # TYPE exec_latency_ms histogram\n\
             exec_latency_ms_bucket{le=\"1\"} 0\n\
             exec_latency_ms_bucket{le=\"10\"} 0\n\
             exec_latency_ms_bucket{le=\"50\"} 0\n\
             exec_latency_ms_bucket{le=\"250\"} 0\n\
             exec_latency_ms_bucket{le=\"1000\"} 0\n\
             exec_latency_ms_bucket{le=\"+Inf\"} 0\n\
             exec_latency_ms_sum 0\n\
             exec_latency_ms_count 0\n"
        );
    }

    #[test]
    fn metrics_render_reflects_recorded_values() {
        let metrics = Metrics::new();
        metrics.record_sandbox_created();
        metrics.record_sandbox_created();
        metrics.record_boot_duration_ms(32.5);
        metrics.record_exec_latency_ms(0.3);

        let text = metrics.render(2);
        assert!(text.contains("sandboxes_created_total 2\n"));
        assert!(text.contains("sandboxes_active 2\n"));
        // 32.5 falls into the (25, 50] bucket and everything above it.
        assert!(text.contains("boot_duration_ms_bucket{le=\"25\"} 0\n"));
        assert!(text.contains("boot_duration_ms_bucket{le=\"50\"} 1\n"));
        assert!(text.contains("boot_duration_ms_bucket{le=\"100\"} 1\n"));
        assert!(text.contains("boot_duration_ms_sum 32.5\n"));
        assert!(text.contains("boot_duration_ms_count 1\n"));
        // 0.3 falls into the (-inf, 1] bucket and everything above it.
        assert!(text.contains("exec_latency_ms_bucket{le=\"1\"} 1\n"));
        assert!(text.contains("exec_latency_ms_bucket{le=\"10\"} 1\n"));
        assert!(text.contains("exec_latency_ms_sum 0.3\n"));
        assert!(text.contains("exec_latency_ms_count 1\n"));
    }

    #[test]
    fn histogram_bucketing_is_inclusive_of_its_upper_bound() {
        let h = Histogram::new(&[10.0, 20.0]);
        h.observe(10.0);
        let mut out = String::new();
        h.render("x", &mut out);
        assert_eq!(
            out,
            "x_bucket{le=\"10\"} 1\n\
             x_bucket{le=\"20\"} 1\n\
             x_bucket{le=\"+Inf\"} 1\n\
             x_sum 10\n\
             x_count 1\n"
        );
    }

    #[test]
    fn histogram_observation_above_all_bounds_only_counts_toward_inf() {
        let h = Histogram::new(&[10.0, 20.0]);
        h.observe(999.0);
        let mut out = String::new();
        h.render("x", &mut out);
        assert_eq!(
            out,
            "x_bucket{le=\"10\"} 0\n\
             x_bucket{le=\"20\"} 0\n\
             x_bucket{le=\"+Inf\"} 1\n\
             x_sum 999\n\
             x_count 1\n"
        );
    }

    #[test]
    fn fmt_bound_drops_trailing_zero_for_whole_numbers() {
        assert_eq!(fmt_bound(50.0), "50");
        assert_eq!(fmt_bound(0.5), "0.5");
    }
}
