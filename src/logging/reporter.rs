use std::backtrace::Backtrace;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use axum::response::Response;
use serde_json::Value;

use crate::auth::Actor;
use crate::events::EventOrigin;
use crate::foundation::AppContext;
use crate::jobs::{JobDeadLetterContext, JobMiddleware};

use super::{current_execution, CurrentRequest, ExecutionContext};

#[async_trait]
pub trait ErrorReporter: Send + Sync + 'static {
    async fn report_handler_error(&self, report: HandlerErrorReport);

    async fn report_panic(&self, report: PanicReport);

    async fn report_job_dead_lettered(&self, report: JobDeadLetteredReport);
}

#[derive(Clone, Debug)]
pub struct HandlerErrorReport {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub error: String,
    pub chain: Vec<String>,
    pub origin: Option<EventOrigin>,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum PanicContext {
    Http {
        request_id: Option<String>,
        method: String,
        path: String,
    },
    Job {
        id: String,
        class: String,
    },
    Scheduler {
        id: String,
    },
    Other,
}

#[derive(Clone, Debug)]
pub struct PanicReport {
    pub message: String,
    pub location: String,
    pub backtrace: Option<String>,
    pub context: PanicContext,
}

#[derive(Clone, Debug)]
pub struct JobDeadLetteredReport {
    pub job_class: String,
    pub job_id: String,
    pub attempts: u32,
    pub last_error: String,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct HandlerErrorResponseExtension {
    status: u16,
    error: String,
    chain: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct ErrorReporterRegistry {
    reporters: Vec<Arc<dyn ErrorReporter>>,
    handler_min_status: u16,
}

impl ErrorReporterRegistry {
    pub(crate) fn new(reporters: Vec<Arc<dyn ErrorReporter>>) -> Self {
        Self {
            reporters,
            handler_min_status: 500,
        }
    }

    pub(crate) async fn report_handler_error(&self, report: HandlerErrorReport) {
        if report.status < self.handler_min_status {
            return;
        }

        for reporter in &self.reporters {
            reporter.report_handler_error(report.clone()).await;
        }
    }

    pub(crate) async fn report_panic(&self, report: PanicReport) {
        for reporter in &self.reporters {
            reporter.report_panic(report.clone()).await;
        }
    }

    pub(crate) async fn report_job_dead_lettered(&self, report: JobDeadLetteredReport) {
        for reporter in &self.reporters {
            reporter.report_job_dead_lettered(report.clone()).await;
        }
    }
}

pub(crate) fn mark_handler_error_response(
    response: &mut Response,
    status: u16,
    error: String,
    chain: Vec<String>,
) {
    response
        .extensions_mut()
        .insert(HandlerErrorResponseExtension {
            status,
            error,
            chain,
        });
}

pub(crate) async fn report_handler_error_response(
    app: &AppContext,
    method: &str,
    path: &str,
    request: &CurrentRequest,
    actor: Option<Actor>,
    extension: Option<HandlerErrorResponseExtension>,
) {
    let Some(extension) = extension else {
        return;
    };

    if extension.status >= 500 {
        tracing::error!(
            method = %method,
            path = %path,
            status = extension.status,
            request_id = ?request.request_id,
            error = %extension.error,
            chain = ?extension.chain,
            "Handler returned server error response"
        );
    } else {
        tracing::warn!(
            method = %method,
            path = %path,
            status = extension.status,
            request_id = ?request.request_id,
            error = %extension.error,
            chain = ?extension.chain,
            "Handler returned client error response"
        );
    }

    let Ok(registry) = app.resolve::<ErrorReporterRegistry>() else {
        return;
    };

    let origin = EventOrigin::from_request(actor, Some(request));
    let report = HandlerErrorReport {
        method: method.to_string(),
        path: path.to_string(),
        status: extension.status,
        error: extension.error,
        chain: extension.chain,
        origin,
        request_id: request.request_id.clone(),
    };

    registry.report_handler_error(report).await;
}

pub(crate) async fn report_job_dead_lettered(app: &AppContext, report: JobDeadLetteredReport) {
    let Ok(registry) = app.resolve::<ErrorReporterRegistry>() else {
        return;
    };

    registry.report_job_dead_lettered(report).await;
}

pub(crate) fn set_global_panic_reporters(registry: Arc<ErrorReporterRegistry>) {
    let slot = GLOBAL_PANIC_REPORTERS.get_or_init(|| Mutex::new(None));
    *slot.lock().expect("panic reporter registry lock poisoned") = Some(registry);
}

pub(crate) fn report_panic_from_hook(message: String, location: String) {
    let registry = GLOBAL_PANIC_REPORTERS
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|guard| guard.clone()));
    let Some(registry) = registry else {
        return;
    };

    let report = PanicReport {
        message,
        location,
        backtrace: Some(Backtrace::force_capture().to_string()),
        context: current_execution()
            .map(panic_context_from_execution)
            .unwrap_or(PanicContext::Other),
    };

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            registry.report_panic(report).await;
        });
    }
}

fn panic_context_from_execution(context: ExecutionContext) -> PanicContext {
    match context {
        ExecutionContext::Http {
            request_id,
            method,
            path,
        } => PanicContext::Http {
            request_id,
            method,
            path,
        },
        ExecutionContext::Job { class, id } => PanicContext::Job { id, class },
        ExecutionContext::Scheduler { id } => PanicContext::Scheduler { id },
        ExecutionContext::Other => PanicContext::Other,
    }
}

static GLOBAL_PANIC_REPORTERS: OnceLock<Mutex<Option<Arc<ErrorReporterRegistry>>>> =
    OnceLock::new();

pub(crate) struct ErrorReporterJobMiddleware;

#[async_trait]
impl JobMiddleware for ErrorReporterJobMiddleware {
    async fn on_dead_lettered(
        &self,
        context: &JobDeadLetterContext,
    ) -> crate::foundation::Result<()> {
        report_job_dead_lettered(
            &context.app,
            JobDeadLetteredReport {
                job_class: context.class.clone(),
                job_id: context.id.clone(),
                attempts: context.attempts,
                last_error: context.last_error.clone(),
                payload: context.payload.clone(),
            },
        )
        .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::auth::Actor;
    use crate::config::ConfigRepository;
    use crate::foundation::Container;
    use crate::support::GuardId;
    use crate::validation::RuleRegistry;

    #[derive(Default)]
    struct StubReporter {
        handler_reports: Mutex<Vec<HandlerErrorReport>>,
        panic_reports: Mutex<Vec<PanicReport>>,
        dead_letter_reports: Mutex<Vec<JobDeadLetteredReport>>,
    }

    #[async_trait]
    impl ErrorReporter for StubReporter {
        async fn report_handler_error(&self, report: HandlerErrorReport) {
            self.handler_reports.lock().unwrap().push(report);
        }

        async fn report_panic(&self, report: PanicReport) {
            self.panic_reports.lock().unwrap().push(report);
        }

        async fn report_job_dead_lettered(&self, report: JobDeadLetteredReport) {
            self.dead_letter_reports.lock().unwrap().push(report);
        }
    }

    fn test_app_with_reporters(reporter: Arc<StubReporter>) -> AppContext {
        let app = AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap();
        let registry = Arc::new(ErrorReporterRegistry::new(vec![reporter]));
        app.container().singleton_arc(registry).unwrap();
        app
    }

    #[tokio::test]
    async fn reports_handler_errors_with_origin() {
        let reporter = Arc::new(StubReporter::default());
        let app = test_app_with_reporters(reporter.clone());
        let request = CurrentRequest {
            request_id: Some("req-handler".to_string()),
            ip: Some("203.0.113.5".parse().unwrap()),
            user_agent: Some("ForgeReporter/1.0".to_string()),
            audit_area: None,
        };

        report_handler_error_response(
            &app,
            "GET",
            "/boom",
            &request,
            Some(Actor::new("admin-1", GuardId::new("admin"))),
            Some(HandlerErrorResponseExtension {
                status: 500,
                error: "boom".to_string(),
                chain: vec!["cause".to_string()],
            }),
        )
        .await;

        let reports = reporter.handler_reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, 500);
        assert_eq!(reports[0].request_id.as_deref(), Some("req-handler"));
        assert_eq!(
            reports[0]
                .origin
                .as_ref()
                .and_then(|origin| origin.actor.as_ref())
                .map(|actor| actor.id.as_str()),
            Some("admin-1")
        );
    }

    #[tokio::test]
    async fn reports_panics_using_execution_context() {
        let reporter = Arc::new(StubReporter::default());
        let registry = Arc::new(ErrorReporterRegistry::new(vec![reporter.clone()]));
        set_global_panic_reporters(registry);

        crate::logging::scope_current_execution(
            ExecutionContext::Job {
                class: "email.send".to_string(),
                id: "job-1".to_string(),
            },
            async {
                report_panic_from_hook("oops".to_string(), "src/tests.rs:1".to_string());
            },
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let reports = reporter.panic_reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        match &reports[0].context {
            PanicContext::Job { id, class } => {
                assert_eq!(id, "job-1");
                assert_eq!(class, "email.send");
            }
            other => panic!("unexpected panic context: {other:?}"),
        }
    }

    #[tokio::test]
    async fn reports_dead_lettered_jobs() {
        let reporter = Arc::new(StubReporter::default());
        let app = test_app_with_reporters(reporter.clone());

        report_job_dead_lettered(
            &app,
            JobDeadLetteredReport {
                job_class: "email.send".to_string(),
                job_id: "job-1".to_string(),
                attempts: 3,
                last_error: "boom".to_string(),
                payload: serde_json::json!({ "email": "hello@example.com" }),
            },
        )
        .await;

        let reports = reporter.dead_letter_reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].job_class, "email.send");
        assert_eq!(reports[0].job_id, "job-1");
    }
}
