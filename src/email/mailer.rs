use std::path::Path;

use crate::foundation::{AppContext, Error, Result};
use crate::storage::StorageManager;

use super::address::EmailAddress;
use super::attachment::{EmailAttachment, ResolvedAttachment};
use super::config::EmailFromConfig;
use super::driver::OutboundEmail;
use super::message::EmailMessage;

#[derive(Clone)]
pub struct EmailMailer {
    app: AppContext,
    mailer_name: Option<String>,
}

impl EmailMailer {
    pub(crate) fn new(app: AppContext, mailer_name: Option<String>) -> Self {
        Self { app, mailer_name }
    }

    pub fn name(&self) -> Option<&str> {
        self.mailer_name.as_deref()
    }

    /// Send immediately: resolve sender + attachments, then call driver.
    pub async fn send(&self, message: EmailMessage) -> Result<()> {
        let manager = self.app.resolve::<super::EmailManager>()?;
        let outbound = self
            .resolve_message(message, manager.from_address())
            .await?;
        let driver = manager.driver(self.mailer_name.as_deref())?;
        driver.send(&outbound).await
    }

    /// Queue for async delivery via Forge jobs.
    pub async fn queue(&self, message: EmailMessage) -> Result<()> {
        let job = super::job::SendQueuedEmailJob {
            mailer_name: self.mailer_name.clone(),
            message,
        };
        let dispatcher = self.app.jobs()?;
        dispatcher.dispatch(job).await
    }

    /// Queue for delayed delivery.
    pub async fn queue_later(&self, message: EmailMessage, run_at_millis: i64) -> Result<()> {
        let job = super::job::SendQueuedEmailJob {
            mailer_name: self.mailer_name.clone(),
            message,
        };
        let dispatcher = self.app.jobs()?;
        dispatcher.dispatch_later(job, run_at_millis).await
    }

    /// Resolve sender fallback: message.from > config email.from > error.
    /// Resolve attachments to bytes. Validate message.
    async fn resolve_message(
        &self,
        message: EmailMessage,
        from_config: &EmailFromConfig,
    ) -> Result<OutboundEmail> {
        if message.to.is_empty() {
            return Err(Error::message("email message has no recipients"));
        }
        if message.text_body.is_none() && message.html_body.is_none() {
            return Err(Error::message(
                "email message has no body (text or html required)",
            ));
        }
        let from = message
            .from
            .or_else(|| {
                if from_config.address.is_empty() {
                    None
                } else {
                    Some(EmailAddress::with_name(
                        &from_config.address,
                        &from_config.name,
                    ))
                }
            })
            .ok_or_else(|| {
                Error::message("no sender address: set message.from or configure [email.from]")
            })?;

        let reply_to = message.reply_to.map(|addr| vec![addr]).unwrap_or_default();

        let mut attachments = Vec::with_capacity(message.attachments.len());
        for att in &message.attachments {
            attachments.push(self.resolve_attachment(att).await?);
        }

        Ok(OutboundEmail {
            from,
            to: message.to,
            cc: message.cc,
            bcc: message.bcc,
            reply_to,
            subject: message.subject,
            text_body: message.text_body,
            html_body: message.html_body,
            headers: message.headers,
            attachments,
        })
    }

    async fn resolve_attachment(&self, att: &EmailAttachment) -> Result<ResolvedAttachment> {
        let (content, fallback_name) = match att {
            EmailAttachment::Path { path, .. } => {
                let bytes = tokio::fs::read(path).await.map_err(|e| {
                    Error::message(format!("failed to read attachment '{}': {e}", path))
                })?;
                (
                    bytes,
                    Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("attachment")
                        .to_string(),
                )
            }
            EmailAttachment::Storage { disk, path, .. } => {
                let storage = self.app.resolve::<StorageManager>()?;
                let bytes = match disk {
                    Some(d) => storage.disk(d)?.get(path).await?,
                    None => storage.default_disk()?.get(path).await?,
                };
                (
                    bytes,
                    Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("attachment")
                        .to_string(),
                )
            }
        };
        let name = att.name().unwrap_or(&fallback_name).to_string();
        let content_type = infer_content_type(&name);
        Ok(ResolvedAttachment {
            content,
            name,
            content_type,
        })
    }
}

fn infer_content_type(name: &str) -> String {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("csv") => "text/csv",
        Some("txt") => "text/plain",
        Some("html") => "text/html",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_content_type_known_extensions() {
        assert_eq!(infer_content_type("report.pdf"), "application/pdf");
        assert_eq!(infer_content_type("photo.png"), "image/png");
        assert_eq!(infer_content_type("photo.jpg"), "image/jpeg");
        assert_eq!(infer_content_type("photo.jpeg"), "image/jpeg");
        assert_eq!(infer_content_type("data.csv"), "text/csv");
        assert_eq!(infer_content_type("readme.txt"), "text/plain");
        assert_eq!(infer_content_type("data.json"), "application/json");
        assert_eq!(infer_content_type("archive.zip"), "application/zip");
    }

    #[test]
    fn infer_content_type_unknown_extension() {
        assert_eq!(infer_content_type("file.xyz"), "application/octet-stream");
        assert_eq!(infer_content_type("file"), "application/octet-stream");
    }
}
