use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use futures_util::FutureExt;

use crate::config::ConfigRepository;
use crate::foundation::{Error, Result};
use crate::logging::panic_payload_message;

use super::{adapter::StorageAdapter, StorageDriverFactory};

pub(crate) async fn build_storage_driver(
    driver: &str,
    factory: &StorageDriverFactory,
    config: &ConfigRepository,
    table: &toml::Table,
) -> Result<Arc<dyn StorageAdapter>> {
    let subject = format!("driver `{driver}` factory");
    let future = catch_unwind(AssertUnwindSafe(|| factory(config, table)))
        .map_err(|panic| storage_panic_error(&subject, panic))?;

    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => Err(storage_panic_error(&subject, panic)),
    }
}

pub(crate) async fn run_storage_operation<F, Fut, T>(
    disk: &str,
    operation: &'static str,
    run: F,
) -> Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let subject = format!("disk `{disk}` {operation}");
    let future = catch_unwind(AssertUnwindSafe(run))
        .map_err(|panic| storage_panic_error(&subject, panic))?;

    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => Err(storage_panic_error(&subject, panic)),
    }
}

fn storage_panic_error(subject: &str, panic: Box<dyn std::any::Any + Send>) -> Error {
    let message = panic_payload_message(panic);
    tracing::error!(
        target: "forge.storage",
        subject = subject,
        panic = %message,
        "storage callback panicked"
    );
    Error::message(format!("storage {subject} panicked: {message}"))
}
