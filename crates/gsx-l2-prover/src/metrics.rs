//! Prometheus metrics for the prover daemon (Track G G4.1, #104).
//!
//! ## Why hand-rolled?
//!
//! Mirrors `gsx-node`'s `metrics_http.rs` posture: the `prometheus`
//! and `prometheus-client` crates each pull ~20 transitive deps, and
//! the text exposition format is dead simple (`# TYPE name kind` +
//! `name value`). The prover's v1 metric set is tiny, so we keep the
//! dep tree lean and auditable for a long-running process.
//!
//! ## Metric set (exact names are part of the #104 contract)
//!
//! - `prove_duration_seconds` (gauge) — wall-clock seconds the most
//!   recent successful proof took. A scaffold gauge; a histogram with
//!   buckets lands once the real SP1 prover (gated on #88/#94) gives
//!   us a meaningful latency distribution to bucket.
//! - `batches_proven_total` (counter) — cumulative count of batches
//!   that proved + submitted successfully.
//! - `prove_failures_total` (counter) — cumulative count of batches
//!   that exhausted their retry budget without producing a proof.

use std::{
    fmt::Write as _,
    sync::atomic::{AtomicU64, Ordering},
};

/// Atomic counters + the last-observed prove duration. Cloneable via
/// `Arc`; the poll loop mutates these while the `/metrics` HTTP
/// handler reads them concurrently.
///
/// `prove_duration_seconds` is stored as the raw `f64` bits in an
/// `AtomicU64` so the whole struct stays lock-free.
#[derive(Debug, Default)]
pub struct Metrics {
    /// `batches_proven_total` counter.
    batches_proven_total: AtomicU64,
    /// `prove_failures_total` counter.
    prove_failures_total: AtomicU64,
    /// `prove_duration_seconds` gauge, stored as `f64::to_bits`.
    prove_duration_seconds_bits: AtomicU64,
}

impl Metrics {
    /// New zeroed metric set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful proof: bump `batches_proven_total` and set
    /// the `prove_duration_seconds` gauge to the observed duration.
    pub fn record_success(&self, duration_seconds: f64) {
        self.batches_proven_total.fetch_add(1, Ordering::Relaxed);
        self.prove_duration_seconds_bits
            .store(duration_seconds.to_bits(), Ordering::Relaxed);
    }

    /// Record a permanent failure (retry budget exhausted): bump
    /// `prove_failures_total`.
    pub fn record_failure(&self) {
        self.prove_failures_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Current `batches_proven_total` value.
    pub fn batches_proven_total(&self) -> u64 {
        self.batches_proven_total.load(Ordering::Relaxed)
    }

    /// Current `prove_failures_total` value.
    pub fn prove_failures_total(&self) -> u64 {
        self.prove_failures_total.load(Ordering::Relaxed)
    }

    /// Current `prove_duration_seconds` gauge value.
    pub fn prove_duration_seconds(&self) -> f64 {
        f64::from_bits(self.prove_duration_seconds_bits.load(Ordering::Relaxed))
    }

    /// Render the metric set as a Prometheus text-exposition body
    /// (`text/plain; version=0.0.4`). Pure function over the current
    /// atomic snapshot so it is trivially unit-testable.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(512);

        let _ = writeln!(
            out,
            "# HELP prove_duration_seconds Wall-clock seconds the most recent successful proof took."
        );
        let _ = writeln!(out, "# TYPE prove_duration_seconds gauge");
        let _ = writeln!(
            out,
            "prove_duration_seconds {}",
            self.prove_duration_seconds()
        );

        let _ = writeln!(
            out,
            "# HELP batches_proven_total Cumulative count of batches proven and submitted successfully."
        );
        let _ = writeln!(out, "# TYPE batches_proven_total counter");
        let _ = writeln!(out, "batches_proven_total {}", self.batches_proven_total());

        let _ = writeln!(
            out,
            "# HELP prove_failures_total Cumulative count of batches that exhausted their retry budget without producing a proof."
        );
        let _ = writeln!(out, "# TYPE prove_failures_total counter");
        let _ = writeln!(out, "prove_failures_total {}", self.prove_failures_total());

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_contains_all_three_metric_names() {
        let m = Metrics::new();
        let body = m.render();
        assert!(body.contains("prove_duration_seconds"));
        assert!(body.contains("batches_proven_total"));
        assert!(body.contains("prove_failures_total"));
        // TYPE lines present so a scraper classifies them correctly.
        assert!(body.contains("# TYPE prove_duration_seconds gauge"));
        assert!(body.contains("# TYPE batches_proven_total counter"));
        assert!(body.contains("# TYPE prove_failures_total counter"));
    }

    #[test]
    fn counters_and_gauge_reflect_recorded_events() {
        let m = Metrics::new();
        assert_eq!(m.batches_proven_total(), 0);
        assert_eq!(m.prove_failures_total(), 0);
        assert_eq!(m.prove_duration_seconds(), 0.0);

        m.record_success(1.5);
        m.record_success(2.25);
        m.record_failure();

        assert_eq!(m.batches_proven_total(), 2);
        assert_eq!(m.prove_failures_total(), 1);
        // Gauge holds the most recent duration, not a sum.
        assert_eq!(m.prove_duration_seconds(), 2.25);

        let body = m.render();
        assert!(body.contains("batches_proven_total 2"));
        assert!(body.contains("prove_failures_total 1"));
        assert!(body.contains("prove_duration_seconds 2.25"));
    }
}
