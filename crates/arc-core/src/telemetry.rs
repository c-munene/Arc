use prometheus::{
    exponential_buckets, register_histogram_vec, register_int_counter_vec, register_int_gauge_vec,
    HistogramVec, IntCounterVec, IntGaugeVec,
};

/// Metrics registry (Prometheus).
#[derive(Clone, Debug)]
pub struct Metrics {
    pub requests_total: IntCounterVec,
    pub bytes_in_total: IntCounterVec,
    pub bytes_out_total: IntCounterVec,
    pub latency_seconds: HistogramVec,
    pub upstream_inflight: IntGaugeVec,
}

impl Metrics {
    pub fn init_global() -> anyhow::Result<Self> {
        let requests_total = register_int_counter_vec!(
            "arc_requests_total",
            "Total requests",
            &["route", "status"]
        )?;
        let bytes_in_total = register_int_counter_vec!(
            "arc_bytes_in_total",
            "Total inbound bytes",
            &["route"]
        )?;
        let bytes_out_total = register_int_counter_vec!(
            "arc_bytes_out_total",
            "Total outbound bytes",
            &["route"]
        )?;

        let buckets = exponential_buckets(0.000_05, 2.0, 20)?; // 50us .. ~50s
        let latency_seconds = register_histogram_vec!(
            "arc_request_latency_seconds",
            "Request latency seconds",
            &["route"],
            buckets
        )?;

        let upstream_inflight = register_int_gauge_vec!(
            "arc_upstream_inflight",
            "Upstream inflight requests",
            &["upstream", "endpoint"]
        )?;

        Ok(Self {
            requests_total,
            bytes_in_total,
            bytes_out_total,
            latency_seconds,
            upstream_inflight,
        })
    }
}
