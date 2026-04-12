use std::sync::OnceLock;

use serde::Deserialize;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::config::ConfigRepository;
use crate::foundation::Result;
use crate::support::{Clock, Timezone};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Json,
    Text,
}

mod diagnostics;
mod file_writer;
mod middleware;
mod observability;
mod probes;
mod request_id;
mod types;

pub use diagnostics::{RuntimeDiagnostics, RuntimeSnapshot};
pub use observability::ObservabilityOptions;
pub use probes::{
    LivenessReport, ProbeResult, ReadinessCheck, ReadinessReport, FRAMEWORK_BOOTSTRAP_PROBE,
    REDIS_PING_PROBE, RUNTIME_BACKEND_PROBE,
};
pub(crate) use probes::{ReadinessRegistryBuilder, ReadinessRegistryHandle};
pub use request_id::{RequestId, REQUEST_ID_HEADER};
pub use types::{
    AuthOutcome, HttpOutcomeClass, JobOutcome, LogLevel, ProbeState, RuntimeBackendKind,
    SchedulerLeadershipState, WebSocketConnectionState,
};

pub(crate) use middleware::request_context_middleware;
pub(crate) use observability::register_observability_routes;

/// Timer that formats timestamps using the framework's configured timezone.
struct ForgeTimer {
    timezone: Timezone,
}

impl ForgeTimer {
    fn new(timezone: Timezone) -> Self {
        Self { timezone }
    }
}

impl FormatTime for ForgeTimer {
    fn format_time(&self, writer: &mut Writer<'_>) -> std::fmt::Result {
        let clock = Clock::new(self.timezone.clone());
        let now = clock.now();
        write!(writer, "{}", now.format_in(&self.timezone))
    }
}

pub fn init(config: &ConfigRepository) -> Result<()> {
    static LOGGING: OnceLock<()> = OnceLock::new();

    if LOGGING.get().is_some() {
        return Ok(());
    }

    let logging_config = config.logging()?;
    let timezone = config.app()?.timezone;
    let level = logging_config.level.as_filter_directive();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    match logging_config.format {
        LogFormat::Json => init_json(filter, &logging_config.log_dir, &timezone)?,
        LogFormat::Text => init_text(filter)?,
    }

    // Panic hook — capture panics as structured error events
    std::panic::set_hook(Box::new(|panic_info| {
        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let message = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        tracing::error!(
            target: "forge.panic",
            location = %location,
            error = %message,
            "Thread panicked"
        );
    }));

    let _ = LOGGING.set(());
    Ok(())
}

fn init_json(filter: EnvFilter, log_dir: &str, timezone: &Timezone) -> Result<()> {
    use crate::foundation::Error;

    let timer = ForgeTimer::new(timezone.clone());
    let clock = Clock::new(timezone.clone());

    if log_dir.is_empty() {
        // stdout only
        let _ = tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_target(true)
            .with_timer(timer)
            .with_env_filter(filter)
            .try_init();
    } else {
        // stdout + date-rotating file
        let file_writer = file_writer::DateRotatingFileWriter::open(log_dir, &clock)
            .map_err(|e| Error::message(format!("failed to open log dir '{log_dir}': {e}")))?;

        let stdout_layer = tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_target(true)
            .with_timer(ForgeTimer::new(timezone.clone()));

        let file_layer = tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_target(true)
            .with_timer(timer)
            .with_writer(file_writer);

        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .with(file_layer)
            .try_init();
    }
    Ok(())
}

fn init_text(filter: EnvFilter) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{
        ProbeResult, ProbeState, ReadinessCheck, ReadinessRegistryBuilder, RuntimeBackendKind,
        RuntimeDiagnostics,
    };
    use crate::config::ConfigRepository;
    use crate::foundation::{AppContext, Container, Error};
    use crate::support::ProbeId;
    use crate::validation::RuleRegistry;

    struct PassingProbe;

    #[async_trait]
    impl ReadinessCheck for PassingProbe {
        async fn run(&self, _app: &AppContext) -> crate::Result<ProbeResult> {
            Ok(ProbeResult::healthy(ProbeId::new("provider.pass")))
        }
    }

    struct FailingProbe;

    #[async_trait]
    impl ReadinessCheck for FailingProbe {
        async fn run(&self, _app: &AppContext) -> crate::Result<ProbeResult> {
            Err(Error::message("not ready"))
        }
    }

    #[test]
    fn rejects_duplicate_probe_registration() {
        let mut builder = ReadinessRegistryBuilder::default();
        builder
            .register_arc(ProbeId::new("database"), Arc::new(PassingProbe))
            .unwrap();
        let error = builder
            .register_arc(ProbeId::new("database"), Arc::new(PassingProbe))
            .unwrap_err();

        assert!(error.to_string().contains("already registered"));
    }

    #[tokio::test]
    async fn readiness_aggregation_reports_failures() {
        let mut builder = ReadinessRegistryBuilder::default();
        builder
            .register_arc(ProbeId::new("provider.pass"), Arc::new(PassingProbe))
            .unwrap();
        builder
            .register_arc(ProbeId::new("provider.fail"), Arc::new(FailingProbe))
            .unwrap();

        let diagnostics = RuntimeDiagnostics::new(
            RuntimeBackendKind::Memory,
            ReadinessRegistryBuilder::freeze_shared(Arc::new(Mutex::new(builder))),
        );
        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let report = diagnostics.run_readiness_checks(&app).await.unwrap();

        assert_eq!(report.state, ProbeState::Unhealthy);
        assert_eq!(report.probes.len(), 2);
        assert_eq!(report.probes[0].state, ProbeState::Healthy);
        assert_eq!(report.probes[1].state, ProbeState::Unhealthy);
        assert_eq!(report.probes[1].id, ProbeId::new("provider.fail"));
    }
}
