mod leadership;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};

use crate::foundation::{AppContext, Error, Result};
use crate::support::ScheduleId;
use crate::support::{boxed, BoxFuture};

pub type ScheduleRegistrar = Arc<dyn Fn(&mut ScheduleRegistry) -> Result<()> + Send + Sync>;
type ScheduleHandler = Arc<dyn Fn(AppContext) -> BoxFuture<Result<()>> + Send + Sync>;

#[derive(Clone)]
pub struct ScheduleInvocation {
    app: AppContext,
}

impl ScheduleInvocation {
    pub(crate) fn new(app: AppContext) -> Self {
        Self { app }
    }

    pub fn app(&self) -> &AppContext {
        &self.app
    }
}

#[derive(Clone)]
pub struct CronExpression {
    source: String,
    schedule: CronSchedule,
}

impl CronExpression {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let source = value.into();
        let schedule = source.parse::<CronSchedule>().map_err(Error::other)?;
        Ok(Self { source, schedule })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub(crate) fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }
}

impl Serialize for CronExpression {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CronExpression {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Self::parse(source).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone)]
pub enum ScheduleKind {
    Cron { expression: Box<CronExpression> },
    Interval { every: Duration },
}

#[derive(Clone)]
pub struct ScheduledTask {
    pub(crate) id: ScheduleId,
    pub(crate) kind: ScheduleKind,
    pub(crate) handler: ScheduleHandler,
}

#[derive(Default)]
pub struct ScheduleRegistry {
    tasks: Vec<ScheduledTask>,
}

impl ScheduleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cron<I, F, Fut>(
        &mut self,
        id: I,
        expression: CronExpression,
        job: F,
    ) -> Result<&mut Self>
    where
        I: Into<ScheduleId>,
        F: Fn(ScheduleInvocation) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let id = id.into();
        ensure_unique_name(&self.tasks, &id)?;

        self.tasks.push(ScheduledTask {
            id,
            kind: ScheduleKind::Cron {
                expression: Box::new(expression),
            },
            handler: Arc::new(move |app| boxed(job(ScheduleInvocation::new(app)))),
        });
        Ok(self)
    }

    pub fn interval<I, F, Fut>(&mut self, id: I, every: Duration, job: F) -> Result<&mut Self>
    where
        I: Into<ScheduleId>,
        F: Fn(ScheduleInvocation) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let id = id.into();
        ensure_unique_name(&self.tasks, &id)?;

        self.tasks.push(ScheduledTask {
            id,
            kind: ScheduleKind::Interval { every },
            handler: Arc::new(move |app| boxed(job(ScheduleInvocation::new(app)))),
        });
        Ok(self)
    }

    pub(crate) fn tasks(self) -> Vec<ScheduledTask> {
        self.tasks
    }
}

fn ensure_unique_name(tasks: &[ScheduledTask], id: &ScheduleId) -> Result<()> {
    if tasks.iter().any(|task| &task.id == id) {
        return Err(Error::message(format!(
            "schedule `{id}` already registered"
        )));
    }
    Ok(())
}

pub(crate) fn cron_due(
    schedule: &CronExpression,
    previous: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    schedule
        .schedule()
        .after(&(previous - chrono::Duration::nanoseconds(1)))
        .next()
        .map(|next| next <= now)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CronExpression, ScheduleRegistry};
    use crate::support::ScheduleId;

    #[test]
    fn rejects_duplicate_schedule_names() {
        let mut registry = ScheduleRegistry::new();
        registry
            .interval(
                ScheduleId::new("heartbeat"),
                Duration::from_secs(5),
                |_invocation| async { Ok(()) },
            )
            .unwrap();

        let error = registry
            .interval(
                ScheduleId::new("heartbeat"),
                Duration::from_secs(5),
                |_invocation| async { Ok(()) },
            )
            .err()
            .unwrap();
        assert!(error.to_string().contains("already registered"));
    }

    #[test]
    fn parses_cron_expressions_before_registration() {
        let expression = CronExpression::parse("*/5 * * * * *").unwrap();

        assert_eq!(expression.as_str(), "*/5 * * * * *");
    }
}
