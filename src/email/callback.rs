use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use futures_util::FutureExt;

use crate::config::ConfigRepository;
use crate::foundation::{Error, Result};
use crate::logging::panic_payload_message;

use super::{EmailDriver, EmailDriverFactory, OutboundEmail};

pub(crate) fn build_email_driver(
    driver: &str,
    factory: &EmailDriverFactory,
    config: &ConfigRepository,
    table: &toml::Table,
) -> Result<Arc<dyn EmailDriver>> {
    let subject = format!("driver `{driver}` factory");
    catch_unwind(AssertUnwindSafe(|| factory(config, table)))
        .map_err(|panic| email_panic_error(&subject, panic))?
}

pub(crate) async fn send_email_driver<F, Fut>(mailer: &str, send: F) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let subject = format!("driver `{mailer}` send");
    let future =
        catch_unwind(AssertUnwindSafe(send)).map_err(|panic| email_panic_error(&subject, panic))?;

    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => Err(email_panic_error(&subject, panic)),
    }
}

pub(crate) async fn send_driver(
    mailer: &str,
    driver: &dyn EmailDriver,
    message: &OutboundEmail,
) -> Result<()> {
    send_email_driver(mailer, || driver.send(message)).await
}

fn email_panic_error(subject: &str, panic: Box<dyn std::any::Any + Send>) -> Error {
    let message = panic_payload_message(panic);
    tracing::error!(
        target: "forge.email",
        subject = subject,
        panic = %message,
        "email callback panicked"
    );
    Error::message(format!("email {subject} panicked: {message}"))
}
