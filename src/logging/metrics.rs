use std::fmt::Write;

use super::diagnostics::RuntimeSnapshot;

/// Format a [`RuntimeSnapshot`] as Prometheus text exposition format.
pub(crate) fn format_prometheus(snapshot: &RuntimeSnapshot) -> String {
    let mut out = String::with_capacity(2048);

    // Bootstrap status (gauge)
    write_gauge(
        &mut out,
        "forge_bootstrap_complete",
        "Whether bootstrap has completed",
        if snapshot.bootstrap_complete { 1 } else { 0 },
    );

    // HTTP request counters
    write_help_type(
        &mut out,
        "forge_http_requests_total",
        "Total HTTP requests handled",
        "counter",
    );
    write_counter_label(
        &mut out,
        "forge_http_requests_total",
        "class",
        "1xx",
        snapshot.http.informational_total,
    );
    write_counter_label(
        &mut out,
        "forge_http_requests_total",
        "class",
        "2xx",
        snapshot.http.success_total,
    );
    write_counter_label(
        &mut out,
        "forge_http_requests_total",
        "class",
        "3xx",
        snapshot.http.redirection_total,
    );
    write_counter_label(
        &mut out,
        "forge_http_requests_total",
        "class",
        "4xx",
        snapshot.http.client_error_total,
    );
    write_counter_label(
        &mut out,
        "forge_http_requests_total",
        "class",
        "5xx",
        snapshot.http.server_error_total,
    );
    write_help_type(
        &mut out,
        "forge_http_request_duration_ms",
        "HTTP request duration histogram in milliseconds",
        "histogram",
    );
    for bucket in &snapshot.http.duration_ms.buckets {
        let _ = writeln!(
            out,
            "forge_http_request_duration_ms_bucket{{le=\"{}\"}} {}",
            bucket.le_ms, bucket.cumulative_count
        );
    }
    let _ = writeln!(
        out,
        "forge_http_request_duration_ms_bucket{{le=\"+Inf\"}} {}",
        snapshot.http.duration_ms.count
    );
    let _ = writeln!(
        out,
        "forge_http_request_duration_ms_sum {}",
        snapshot.http.duration_ms.sum_ms
    );
    let _ = writeln!(
        out,
        "forge_http_request_duration_ms_count {}",
        snapshot.http.duration_ms.count
    );

    // Auth counters
    write_help_type(
        &mut out,
        "forge_auth_total",
        "Total authentication outcomes",
        "counter",
    );
    write_counter_label(
        &mut out,
        "forge_auth_total",
        "outcome",
        "success",
        snapshot.auth.success_total,
    );
    write_counter_label(
        &mut out,
        "forge_auth_total",
        "outcome",
        "unauthorized",
        snapshot.auth.unauthorized_total,
    );
    write_counter_label(
        &mut out,
        "forge_auth_total",
        "outcome",
        "forbidden",
        snapshot.auth.forbidden_total,
    );
    write_counter_label(
        &mut out,
        "forge_auth_total",
        "outcome",
        "error",
        snapshot.auth.error_total,
    );

    // WebSocket counters
    write_help_type(
        &mut out,
        "forge_websocket_connections_total",
        "Total WebSocket connections opened",
        "counter",
    );
    let _ = writeln!(
        out,
        "forge_websocket_connections_total {}",
        snapshot.websocket.opened_total
    );
    write_gauge(
        &mut out,
        "forge_websocket_active_connections",
        "Currently active WebSocket connections",
        snapshot.websocket.active_connections,
    );

    write_help_type(
        &mut out,
        "forge_websocket_messages_total",
        "Total WebSocket messages",
        "counter",
    );
    write_counter_label(
        &mut out,
        "forge_websocket_messages_total",
        "direction",
        "inbound",
        snapshot.websocket.inbound_messages_total,
    );
    write_counter_label(
        &mut out,
        "forge_websocket_messages_total",
        "direction",
        "outbound",
        snapshot.websocket.outbound_messages_total,
    );

    // Per-channel WebSocket series
    write_help_type(
        &mut out,
        "forge_websocket_subscriptions_total",
        "Total WebSocket subscriptions per channel",
        "counter",
    );
    write_help_type(
        &mut out,
        "forge_websocket_active_subscriptions",
        "Currently active WebSocket subscriptions per channel",
        "gauge",
    );
    write_help_type(
        &mut out,
        "forge_websocket_channel_messages_total",
        "Total WebSocket messages per channel",
        "counter",
    );
    for channel in &snapshot.websocket.channels {
        let id = channel.id.as_str();
        let _ = writeln!(
            out,
            "forge_websocket_subscriptions_total{{channel=\"{id}\"}} {}",
            channel.subscriptions_total
        );
        let _ = writeln!(
            out,
            "forge_websocket_active_subscriptions{{channel=\"{id}\"}} {}",
            channel.active_subscriptions
        );
        let _ = writeln!(
            out,
            "forge_websocket_channel_messages_total{{channel=\"{id}\",direction=\"inbound\"}} {}",
            channel.inbound_messages_total
        );
        let _ = writeln!(
            out,
            "forge_websocket_channel_messages_total{{channel=\"{id}\",direction=\"outbound\"}} {}",
            channel.outbound_messages_total
        );
    }

    // Scheduler counters
    write_help_type(
        &mut out,
        "forge_scheduler_ticks_total",
        "Total scheduler ticks",
        "counter",
    );
    let _ = writeln!(
        out,
        "forge_scheduler_ticks_total {}",
        snapshot.scheduler.ticks_total
    );
    write_help_type(
        &mut out,
        "forge_scheduler_executions_total",
        "Total scheduled tasks executed",
        "counter",
    );
    let _ = writeln!(
        out,
        "forge_scheduler_executions_total {}",
        snapshot.scheduler.executed_schedules_total
    );
    write_gauge(
        &mut out,
        "forge_scheduler_leader_active",
        "Whether this instance is the active scheduler leader",
        if snapshot.scheduler.leader_active {
            1
        } else {
            0
        },
    );

    // Job counters
    write_help_type(
        &mut out,
        "forge_jobs_total",
        "Total job lifecycle events",
        "counter",
    );
    write_counter_label(
        &mut out,
        "forge_jobs_total",
        "outcome",
        "enqueued",
        snapshot.jobs.enqueued_total,
    );
    write_counter_label(
        &mut out,
        "forge_jobs_total",
        "outcome",
        "started",
        snapshot.jobs.started_total,
    );
    write_counter_label(
        &mut out,
        "forge_jobs_total",
        "outcome",
        "succeeded",
        snapshot.jobs.succeeded_total,
    );
    write_counter_label(
        &mut out,
        "forge_jobs_total",
        "outcome",
        "retried",
        snapshot.jobs.retried_total,
    );
    write_counter_label(
        &mut out,
        "forge_jobs_total",
        "outcome",
        "dead_lettered",
        snapshot.jobs.dead_lettered_total,
    );

    out
}

fn write_help_type(out: &mut String, name: &str, help: &str, metric_type: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {metric_type}");
}

fn write_gauge(out: &mut String, name: &str, help: &str, value: u64) {
    write_help_type(out, name, help, "gauge");
    let _ = writeln!(out, "{name} {value}");
}

fn write_counter_label(out: &mut String, name: &str, label: &str, label_value: &str, value: u64) {
    let _ = writeln!(out, "{name}{{{label}=\"{label_value}\"}} {value}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::diagnostics::{
        AuthRuntimeSnapshot, HttpDurationBucketSnapshot, HttpDurationHistogramSnapshot,
        HttpRuntimeSnapshot, JobRuntimeSnapshot, RuntimeSnapshot, SchedulerRuntimeSnapshot,
        WebSocketRuntimeSnapshot,
    };
    use crate::logging::types::RuntimeBackendKind;

    #[test]
    fn formats_prometheus_text() {
        let snapshot = RuntimeSnapshot {
            backend: RuntimeBackendKind::Memory,
            bootstrap_complete: true,
            http: HttpRuntimeSnapshot {
                requests_total: 100,
                informational_total: 0,
                success_total: 80,
                redirection_total: 5,
                client_error_total: 10,
                server_error_total: 5,
                duration_ms: HttpDurationHistogramSnapshot {
                    count: 100,
                    sum_ms: 12_345,
                    buckets: vec![
                        HttpDurationBucketSnapshot {
                            le_ms: 5,
                            cumulative_count: 3,
                        },
                        HttpDurationBucketSnapshot {
                            le_ms: 10,
                            cumulative_count: 9,
                        },
                        HttpDurationBucketSnapshot {
                            le_ms: 25,
                            cumulative_count: 25,
                        },
                    ],
                },
            },
            auth: AuthRuntimeSnapshot {
                success_total: 50,
                unauthorized_total: 3,
                forbidden_total: 1,
                error_total: 0,
            },
            websocket: WebSocketRuntimeSnapshot {
                opened_total: 10,
                closed_total: 5,
                active_connections: 5,
                subscriptions_total: 20,
                unsubscribes_total: 10,
                active_subscriptions: 10,
                inbound_messages_total: 100,
                outbound_messages_total: 200,
                channels: Vec::new(),
            },
            scheduler: SchedulerRuntimeSnapshot {
                ticks_total: 500,
                executed_schedules_total: 42,
                leadership_acquired_total: 2,
                leadership_lost_total: 1,
                leader_active: true,
            },
            jobs: JobRuntimeSnapshot {
                enqueued_total: 30,
                leased_total: 28,
                started_total: 28,
                succeeded_total: 25,
                retried_total: 2,
                expired_requeues_total: 1,
                dead_lettered_total: 0,
            },
        };

        let output = format_prometheus(&snapshot);

        assert!(output.contains("forge_bootstrap_complete 1"));
        assert!(output.contains("forge_http_requests_total{class=\"2xx\"} 80"));
        assert!(output.contains("forge_http_requests_total{class=\"5xx\"} 5"));
        assert!(output.contains("# TYPE forge_http_request_duration_ms histogram"));
        assert!(output.contains("forge_http_request_duration_ms_bucket{le=\"25\"} 25"));
        assert!(output.contains("forge_http_request_duration_ms_bucket{le=\"+Inf\"} 100"));
        assert!(output.contains("forge_http_request_duration_ms_sum 12345"));
        assert!(output.contains("forge_http_request_duration_ms_count 100"));
        assert!(output.contains("forge_auth_total{outcome=\"success\"} 50"));
        assert!(output.contains("forge_websocket_active_connections 5"));
        assert!(output.contains("forge_jobs_total{outcome=\"succeeded\"} 25"));
        assert!(output.contains("forge_scheduler_leader_active 1"));
        assert!(output.contains("# TYPE forge_http_requests_total counter"));
        assert!(output.contains("# TYPE forge_bootstrap_complete gauge"));
    }

    #[test]
    fn format_prometheus_emits_per_channel_websocket_series() {
        use crate::logging::diagnostics::WebSocketChannelSnapshot;
        use crate::support::ChannelId;

        let snapshot = RuntimeSnapshot {
            backend: RuntimeBackendKind::Memory,
            bootstrap_complete: false,
            http: HttpRuntimeSnapshot {
                requests_total: 0,
                informational_total: 0,
                success_total: 0,
                redirection_total: 0,
                client_error_total: 0,
                server_error_total: 0,
                duration_ms: HttpDurationHistogramSnapshot {
                    count: 0,
                    sum_ms: 0,
                    buckets: Vec::new(),
                },
            },
            auth: AuthRuntimeSnapshot {
                success_total: 0,
                unauthorized_total: 0,
                forbidden_total: 0,
                error_total: 0,
            },
            websocket: WebSocketRuntimeSnapshot {
                opened_total: 0,
                closed_total: 0,
                active_connections: 5,
                subscriptions_total: 0,
                unsubscribes_total: 0,
                active_subscriptions: 0,
                inbound_messages_total: 0,
                outbound_messages_total: 0,
                channels: vec![WebSocketChannelSnapshot {
                    id: ChannelId::new("chat"),
                    subscriptions_total: 10,
                    unsubscribes_total: 2,
                    active_subscriptions: 8,
                    inbound_messages_total: 100,
                    outbound_messages_total: 300,
                }],
            },
            scheduler: SchedulerRuntimeSnapshot {
                ticks_total: 0,
                executed_schedules_total: 0,
                leadership_acquired_total: 0,
                leadership_lost_total: 0,
                leader_active: false,
            },
            jobs: JobRuntimeSnapshot {
                enqueued_total: 0,
                leased_total: 0,
                started_total: 0,
                succeeded_total: 0,
                retried_total: 0,
                expired_requeues_total: 0,
                dead_lettered_total: 0,
            },
        };

        let output = format_prometheus(&snapshot);

        assert!(output.contains("forge_websocket_active_connections 5"));
        assert!(
            output.contains("forge_websocket_subscriptions_total{channel=\"chat\"} 10"),
            "missing per-channel subscriptions series:\n{output}"
        );
        assert!(output.contains("forge_websocket_active_subscriptions{channel=\"chat\"} 8"));
        assert!(output.contains(
            "forge_websocket_channel_messages_total{channel=\"chat\",direction=\"inbound\"} 100"
        ));
        assert!(output.contains(
            "forge_websocket_channel_messages_total{channel=\"chat\",direction=\"outbound\"} 300"
        ));
    }
}
