use clap::{Arg, ArgAction, Command};
use serde::Serialize;
use serde_json::{json, Value};

use crate::cli::{CommandInvocation, CommandRegistrar};
use crate::database::lifecycle::migration_status_summary_from_app;
use crate::foundation::{AppContext, Error, Result};
use crate::logging::ProbeState;
use crate::support::runtime::RuntimeBackend;
use crate::support::CommandId;

const DOCTOR_COMMAND: CommandId = CommandId::new("doctor");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DoctorStatus {
    Ok,
    Warning,
    Failed,
}

impl DoctorStatus {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Failed, _) | (_, Self::Failed) => Self::Failed,
            (Self::Warning, _) | (_, Self::Warning) => Self::Warning,
            (Self::Ok, Self::Ok) => Self::Ok,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: DoctorStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl DoctorCheck {
    fn ok(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorStatus::Ok,
            message: message.into(),
            details: None,
        }
    }

    fn ok_with_details(name: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            name,
            status: DoctorStatus::Ok,
            message: message.into(),
            details: Some(details),
        }
    }

    fn warning(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorStatus::Warning,
            message: message.into(),
            details: None,
        }
    }

    fn failed(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorStatus::Failed,
            message: message.into(),
            details: None,
        }
    }

    fn failed_with_details(name: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            name,
            status: DoctorStatus::Failed,
            message: message.into(),
            details: Some(details),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct DoctorReport {
    status: DoctorStatus,
    deploy: bool,
    checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn new(deploy: bool) -> Self {
        Self {
            status: DoctorStatus::Ok,
            deploy,
            checks: Vec::new(),
        }
    }

    fn push(&mut self, check: DoctorCheck) {
        self.status = self.status.merge(check.status);
        self.checks.push(check);
    }

    fn failed(&self) -> bool {
        matches!(self.status, DoctorStatus::Failed)
    }
}

pub(crate) fn doctor_cli_registrar() -> CommandRegistrar {
    std::sync::Arc::new(|registry| {
        registry.command(
            DOCTOR_COMMAND,
            Command::new(DOCTOR_COMMAND.as_str().to_string())
                .about("Run runtime health checks for deploy and operator diagnostics")
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Print machine-readable JSON"),
                )
                .arg(
                    Arg::new("deploy")
                        .long("deploy")
                        .action(ArgAction::SetTrue)
                        .help("Run checks expected by runtime-only deploy tooling"),
                ),
            |invocation| async move { doctor_command(invocation).await },
        )?;
        Ok(())
    })
}

async fn doctor_command(invocation: CommandInvocation) -> Result<()> {
    let json = invocation.matches().get_flag("json");
    let deploy = invocation.matches().get_flag("deploy");
    let report = run_doctor(invocation.app(), deploy).await;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(Error::other)?
        );
    } else {
        print_text_report(&report);
    }

    if report.failed() {
        return Err(Error::message("doctor failed: one or more checks failed"));
    }

    Ok(())
}

async fn run_doctor(app: &AppContext, deploy: bool) -> DoctorReport {
    let mut report = DoctorReport::new(deploy);
    report.push(check_app_config(app));
    report.push(check_database(app).await);
    report.push(check_migrations(app).await);
    report.push(check_runtime_backend(app).await);
    report.push(check_readiness(app).await);
    report
}

fn check_app_config(app: &AppContext) -> DoctorCheck {
    match app.config().app() {
        Ok(config) => DoctorCheck::ok_with_details(
            "config",
            format!("app `{}` loaded for `{}`", config.name, config.environment),
            json!({
                "name": config.name,
                "environment": config.environment.to_string(),
                "timezone": config.timezone.to_string(),
            }),
        ),
        Err(error) => DoctorCheck::failed("config", error.to_string()),
    }
}

async fn check_database(app: &AppContext) -> DoctorCheck {
    let database = match app.database() {
        Ok(database) => database,
        Err(error) => return DoctorCheck::failed("database", error.to_string()),
    };

    if !database.is_configured() {
        return DoctorCheck::warning("database", "database is not configured");
    }

    match database.ping().await {
        Ok(()) => DoctorCheck::ok("database", "database ping succeeded"),
        Err(error) => DoctorCheck::failed("database", error.to_string()),
    }
}

async fn check_migrations(app: &AppContext) -> DoctorCheck {
    let database = match app.database() {
        Ok(database) => database,
        Err(error) => return DoctorCheck::failed("migrations", error.to_string()),
    };

    if !database.is_configured() {
        return DoctorCheck::warning(
            "migrations",
            "migration status skipped; database is not configured",
        );
    }

    match migration_status_summary_from_app(app).await {
        Ok(summary) if summary.missing_applied == 0 => DoctorCheck::ok_with_details(
            "migrations",
            format!(
                "{} registered, {} applied, {} pending",
                summary.registered, summary.applied, summary.pending
            ),
            json!(summary),
        ),
        Ok(summary) => DoctorCheck::failed_with_details(
            "migrations",
            format!(
                "{} applied migration(s) are missing from the current binary",
                summary.missing_applied
            ),
            json!(summary),
        ),
        Err(error) => DoctorCheck::failed("migrations", error.to_string()),
    }
}

async fn check_runtime_backend(app: &AppContext) -> DoctorCheck {
    let backend = match app.resolve::<RuntimeBackend>() {
        Ok(backend) => backend,
        Err(error) => return DoctorCheck::failed("runtime_backend", error.to_string()),
    };
    let kind = backend.kind();

    match backend.ping().await {
        Ok(()) => DoctorCheck::ok_with_details(
            "runtime_backend",
            format!("{kind:?} runtime backend ping succeeded"),
            json!({ "kind": kind }),
        ),
        Err(error) => DoctorCheck::failed_with_details(
            "runtime_backend",
            error.to_string(),
            json!({ "kind": kind }),
        ),
    }
}

async fn check_readiness(app: &AppContext) -> DoctorCheck {
    let diagnostics = match app.diagnostics() {
        Ok(diagnostics) => diagnostics,
        Err(error) => return DoctorCheck::failed("readiness", error.to_string()),
    };

    match diagnostics.run_readiness_checks(app).await {
        Ok(report) if report.state == ProbeState::Healthy => {
            DoctorCheck::ok_with_details("readiness", "readiness checks are healthy", json!(report))
        }
        Ok(report) => DoctorCheck::failed_with_details(
            "readiness",
            "one or more readiness checks are unhealthy",
            json!(report),
        ),
        Err(error) => DoctorCheck::failed("readiness", error.to_string()),
    }
}

fn print_text_report(report: &DoctorReport) {
    println!("Forge doctor: {}", report.status.as_str());
    for check in &report.checks {
        println!(
            "[{}] {}: {}",
            check.status.as_str(),
            check.name,
            check.message
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{DoctorReport, DoctorStatus};
    use crate::foundation::App;

    #[test]
    fn report_status_tracks_worst_check_status() {
        let mut report = DoctorReport::new(true);
        assert_eq!(report.status, DoctorStatus::Ok);

        report.push(super::DoctorCheck::warning("warn", "warning"));
        assert_eq!(report.status, DoctorStatus::Warning);

        report.push(super::DoctorCheck::failed("fail", "failed"));
        assert_eq!(report.status, DoctorStatus::Failed);

        report.push(super::DoctorCheck::ok("ok", "ok"));
        assert_eq!(report.status, DoctorStatus::Failed);
    }

    #[tokio::test]
    async fn default_app_doctor_reports_warnings_without_failure() {
        let kernel = App::builder().build_cli_kernel().await.unwrap();
        let report = super::run_doctor(kernel.app(), true).await;

        assert_eq!(report.status, DoctorStatus::Warning);
        assert!(report.deploy);
        assert!(report
            .checks
            .iter()
            .any(|check| check.name == "database" && check.status == DoctorStatus::Warning));
        assert!(report
            .checks
            .iter()
            .any(|check| check.name == "migrations" && check.status == DoctorStatus::Warning));
        assert!(!report.failed());
    }

    #[test]
    fn doctor_report_json_includes_deploy_flag_and_status() {
        let mut report = DoctorReport::new(true);
        report.push(super::DoctorCheck::ok("config", "loaded"));

        let value = serde_json::to_value(report).unwrap();

        assert_eq!(value["status"], "ok");
        assert_eq!(value["deploy"], true);
        assert_eq!(value["checks"][0]["name"], "config");
    }
}
