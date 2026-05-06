use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};

use futures_util::FutureExt;

use crate::foundation::{Error, Result};
use crate::logging::panic_payload_message;

pub(crate) fn run_attachment_sync<T, F>(subject: &str, run: F) -> Result<T>
where
    F: FnOnce() -> T,
{
    catch_unwind(AssertUnwindSafe(run)).map_err(|panic| attachment_panic_error(subject, panic))
}

pub(crate) async fn run_attachment_callback<F, Fut>(subject: &str, run: F) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let future = catch_unwind(AssertUnwindSafe(run))
        .map_err(|panic| attachment_panic_error(subject, panic))?;

    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => Err(attachment_panic_error(subject, panic)),
    }
}

fn attachment_panic_error(subject: &str, panic: Box<dyn std::any::Any + Send>) -> Error {
    let message = panic_payload_message(panic);
    tracing::error!(
        target: "forge.attachments",
        subject = subject,
        panic = %message,
        "attachment callback panicked"
    );
    Error::message(format!("attachment {subject} panicked: {message}"))
}
