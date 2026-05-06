use std::any::Any;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};

use futures_util::FutureExt;

use crate::foundation::{Error, Result};
use crate::logging::panic_payload_message;

pub(crate) fn catch_datatable_callback<T>(
    subject: impl Into<String>,
    callback: impl FnOnce() -> T,
) -> Result<T> {
    let subject = subject.into();
    catch_unwind(AssertUnwindSafe(callback))
        .map_err(|panic| datatable_callback_panic_error(subject, panic))
}

pub(crate) async fn catch_datatable_future<T, Fut>(
    subject: impl Into<String>,
    future: Fut,
) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
{
    let subject = subject.into();
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => Err(datatable_callback_panic_error(subject, panic)),
    }
}

fn datatable_callback_panic_error(subject: String, panic: Box<dyn Any + Send>) -> Error {
    let message = panic_payload_message(panic);
    tracing::error!(
        target: "forge.datatable",
        callback = %subject,
        panic = %message,
        "datatable callback panicked"
    );
    Error::message(format!("datatable {subject} panicked: {message}"))
}
