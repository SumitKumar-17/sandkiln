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
    /// Indexed by `CreatePhase as usize`, parallel to `CreatePhase::ALL`.
    create_phase_duration_ms: Vec<Histogram>,
}

/// The sub-phases of a cold `POST /sandboxes` that get their own
/// timeseries, rendered as one `create_phase_duration_ms` histogram
/// family with a `phase` label rather than one metric name each — adding
/// a phase later is then a variant here, not a new field plus a new
/// render block plus a new test assertion.
///
/// `Boot` is deliberately *not* a variant: `boot_duration_ms` already
/// exists as its own metric and is the one create-related timing anything
/// external could already be scraping, so it stays where it is instead of
/// being duplicated or moved into this family.
/// Variants are indexed positionally into `Metrics::create_phase_duration_ms`
/// via `phase as usize`, so declaration order must match `ALL` — asserted
/// by `create_phase_discriminants_match_all_ordering` below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CreatePhase {
    /// `clone_rootfs` — `cp --reflink=auto` of the base image.
    RootfsClone,
    /// `NetworkManager::lease` — tap/IP pool pops plus `attach_tap`'s
    /// `ip`/`bridge` subprocess calls.
    NetworkLease,
    /// The concurrent join of the two phases above. **Not** their sum:
    /// they run on two threads, so this is roughly the slower of the two
    /// and is the only part of either that actually lands on the create's
    /// critical path.
    Setup,
    /// The whole cold-create path, from request-validated to the sandbox
    /// being inserted into the live map.
    Total,
}

impl CreatePhase {
    pub const ALL: [CreatePhase; 4] = [CreatePhase::RootfsClone, CreatePhase::NetworkLease, CreatePhase::Setup, CreatePhase::Total];

    fn label(self) -> &'static str {
        match self {
            CreatePhase::RootfsClone => "rootfs_clone",
            CreatePhase::NetworkLease => "network_lease",
            CreatePhase::Setup => "setup",
            CreatePhase::Total => "total",
        }
    }
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
            // One shared bucket layout across every phase: the phases
            // span roughly 1ms (a CoW rootfs clone) to a few hundred ms
            // (a whole create), and a per-phase layout would make the
            // family impossible to aggregate over in a single query,
            // which is most of the point of labelling them.
            create_phase_duration_ms: CreatePhase::ALL
                .iter()
                .map(|_| Histogram::new(&[1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 1000.0]))
                .collect(),
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

    pub fn record_create_phase_ms(&self, phase: CreatePhase, ms: f64) {
        self.create_phase_duration_ms[phase as usize].observe(ms);
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

        out.push_str(
            "# HELP create_phase_duration_ms Duration of one sub-phase of a cold sandbox create, in milliseconds. \
             rootfs_clone and network_lease run concurrently and do not sum to setup; boot is reported separately as boot_duration_ms.\n",
        );
        out.push_str("# TYPE create_phase_duration_ms histogram\n");
        for (phase, histogram) in CreatePhase::ALL.iter().zip(self.create_phase_duration_ms.iter()) {
            histogram.render_labeled("create_phase_duration_ms", &format!("phase=\"{}\"", phase.label()), &mut out);
        }

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
        self.render_labeled(name, "", out);
    }

    /// `labels` is a pre-formatted, comma-free label fragment (e.g.
    /// `phase="setup"`) shared by every line this emits — prepended to
    /// the `le` label on bucket lines and used alone on `_sum`/`_count`,
    /// which carry no `le`. Empty means an unlabelled metric, which is
    /// exactly what `render` is.
    fn render_labeled(&self, name: &str, labels: &str, out: &mut String) {
        let sep = if labels.is_empty() { "" } else { "," };
        let alone = if labels.is_empty() { String::new() } else { format!("{{{labels}}}") };

        let state = self.state.lock().unwrap();
        let mut cumulative = 0u64;
        for (bound, &bucket_count) in self.bounds.iter().zip(state.bucket_counts.iter()) {
            cumulative += bucket_count;
            out.push_str(&format!("{name}_bucket{{{labels}{sep}le=\"{}\"}} {cumulative}\n", fmt_bound(*bound)));
        }
        out.push_str(&format!("{name}_bucket{{{labels}{sep}le=\"+Inf\"}} {}\n", state.count));
        out.push_str(&format!("{name}_sum{alone} {}\n", fmt_bound(state.sum)));
        out.push_str(&format!("{name}_count{alone} {}\n", state.count));
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
        let rendered = metrics.render(0);
        // Exact-prefix rather than exact-equals: the
        // `create_phase_duration_ms` family below it is four labelled
        // repetitions of the same eleven lines, checked separately by
        // `create_phase_family_renders_one_labelled_series_per_phase`
        // instead of pasted out in full here.
        let rest = rendered.strip_prefix(
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
             exec_latency_ms_count 0\n",
        );
        assert!(rest.is_some(), "unexpected rendering of the pre-create-phase metrics:\n{rendered}");
    }

    #[test]
    fn create_phase_family_renders_one_labelled_series_per_phase() {
        let metrics = Metrics::new();
        metrics.record_create_phase_ms(CreatePhase::NetworkLease, 7.5);

        let text = metrics.render(0);
        for phase in CreatePhase::ALL {
            assert!(
                text.contains(&format!("create_phase_duration_ms_count{{phase=\"{}\"}} ", phase.label())),
                "phase {phase:?} is missing from the rendered family:\n{text}"
            );
        }
        // 7.5 lands in the (5, 10] bucket of the phase it was recorded
        // against, and nowhere in any other phase's series.
        assert!(text.contains("create_phase_duration_ms_bucket{phase=\"network_lease\",le=\"5\"} 0\n"));
        assert!(text.contains("create_phase_duration_ms_bucket{phase=\"network_lease\",le=\"10\"} 1\n"));
        assert!(text.contains("create_phase_duration_ms_sum{phase=\"network_lease\"} 7.5\n"));
        assert!(text.contains("create_phase_duration_ms_count{phase=\"network_lease\"} 1\n"));
        assert!(text.contains("create_phase_duration_ms_count{phase=\"total\"} 0\n"));
    }

    /// `record_create_phase_ms` indexes its histogram vec by
    /// `phase as usize`, which is only correct while the enum's
    /// declaration order matches `ALL`'s.
    #[test]
    fn create_phase_discriminants_match_all_ordering() {
        for (i, phase) in CreatePhase::ALL.into_iter().enumerate() {
            assert_eq!(phase as usize, i, "{phase:?} is out of order relative to CreatePhase::ALL");
        }
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
