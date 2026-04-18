mod channel;
pub(crate) mod job;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::email::EmailMessage;
use crate::foundation::{AppContext, Error, Result};
use crate::support::NotificationChannelId;

pub use channel::{
    BroadcastNotificationChannel, DatabaseNotificationChannel, EmailNotificationChannel,
    NotificationChannel,
};
pub use job::SendNotificationJob;

// ---------------------------------------------------------------------------
// Core Traits
// ---------------------------------------------------------------------------

/// A notification that can be sent across multiple channels.
///
/// Consumer implements this for each notification type.
///
/// ```ignore
/// impl Notification for OrderShipped {
///     fn notification_type(&self) -> &str { "order_shipped" }
///     fn via(&self) -> Vec<String> { vec!["email".into(), "database".into()] }
///     fn to_email(&self, notifiable: &dyn Notifiable) -> Option<EmailMessage> {
///         let email = notifiable.route_notification_for("email")?;
///         Some(EmailMessage::new("Order shipped!").to(&email).text("Your order is on its way."))
///     }
/// }
/// ```
pub trait Notification: Send + Sync {
    /// A stable type identifier for this notification (stored in DB).
    fn notification_type(&self) -> &str;

    /// Which channels to deliver to (e.g., `[NOTIFY_EMAIL, NOTIFY_DATABASE]`).
    fn via(&self) -> Vec<NotificationChannelId>;

    /// Render as an email message.
    fn to_email(&self, _notifiable: &dyn Notifiable) -> Option<EmailMessage> {
        None
    }

    /// Render as a database record (JSON stored in `notifications.data`).
    fn to_database(&self) -> Option<serde_json::Value> {
        None
    }

    /// Render as a WebSocket broadcast payload.
    fn to_broadcast(&self) -> Option<serde_json::Value> {
        None
    }

    /// Render for a custom channel. Called when the channel name doesn't match
    /// a built-in `to_*` method.
    fn to_channel(
        &self,
        _channel: &str,
        _notifiable: &dyn Notifiable,
    ) -> Option<serde_json::Value> {
        None
    }
}

/// A model that can receive notifications.
///
/// ```ignore
/// impl Notifiable for User {
///     fn notification_id(&self) -> String { self.id.to_string() }
///     fn route_notification_for(&self, channel: &str) -> Option<String> {
///         match channel {
///             "email" => Some(self.email.clone()),
///             "sms" => self.phone.clone(),
///             _ => None,
///         }
///     }
/// }
/// ```
pub trait Notifiable: Send + Sync {
    /// Unique identifier for this notifiable entity (e.g., user ID).
    fn notification_id(&self) -> String;

    /// Return the routing address for a given channel (e.g., email address, phone number).
    fn route_notification_for(&self, _channel: &str) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// Channel Registry
// ---------------------------------------------------------------------------

pub(crate) type NotificationChannelRegistryHandle = Arc<Mutex<NotificationChannelRegistryBuilder>>;

#[derive(Default)]
pub(crate) struct NotificationChannelRegistryBuilder {
    channels: HashMap<NotificationChannelId, Arc<dyn NotificationChannel>>,
}

impl NotificationChannelRegistryBuilder {
    pub(crate) fn shared() -> NotificationChannelRegistryHandle {
        Arc::new(Mutex::new(Self::default()))
    }

    pub(crate) fn contains(&self, id: &NotificationChannelId) -> bool {
        self.channels.contains_key(id)
    }

    pub(crate) fn register<I>(&mut self, id: I, channel: Arc<dyn NotificationChannel>) -> Result<()>
    where
        I: Into<NotificationChannelId>,
    {
        let id = id.into();
        if self.channels.contains_key(&id) {
            return Err(Error::message(format!(
                "notification channel `{id}` already registered"
            )));
        }
        self.channels.insert(id, channel);
        Ok(())
    }

    pub(crate) fn freeze_shared(
        handle: NotificationChannelRegistryHandle,
    ) -> NotificationChannelRegistry {
        let mut builder = handle
            .lock()
            .expect("notification channel registry lock poisoned");
        NotificationChannelRegistry {
            channels: std::mem::take(&mut builder.channels),
        }
    }
}

/// Registry of notification channel adapters, frozen at boot.
pub struct NotificationChannelRegistry {
    channels: HashMap<NotificationChannelId, Arc<dyn NotificationChannel>>,
}

impl NotificationChannelRegistry {
    /// Look up a channel by ID.
    pub fn get(&self, id: &NotificationChannelId) -> Option<&Arc<dyn NotificationChannel>> {
        self.channels.get(id)
    }
}

/// Well-known built-in channel IDs.
pub const NOTIFY_EMAIL: NotificationChannelId = NotificationChannelId::new("email");
pub const NOTIFY_DATABASE: NotificationChannelId = NotificationChannelId::new("database");
pub const NOTIFY_BROADCAST: NotificationChannelId = NotificationChannelId::new("broadcast");

// ---------------------------------------------------------------------------
// Dispatch Functions
// ---------------------------------------------------------------------------

/// Send a notification synchronously (all channels await'd in sequence).
pub async fn notify(
    app: &AppContext,
    notifiable: &dyn Notifiable,
    notification: &dyn Notification,
) -> Result<()> {
    let registry = app.resolve::<NotificationChannelRegistry>()?;

    for channel_id in notification.via() {
        if let Some(channel) = registry.get(&channel_id) {
            if let Err(error) = channel.send(app, notifiable, notification).await {
                tracing::error!(
                    channel = %channel_id,
                    notification_type = %notification.notification_type(),
                    error = %error,
                    "notification channel delivery failed"
                );
            }
        } else {
            tracing::warn!(
                channel = %channel_id,
                "notification channel not registered, skipping"
            );
        }
    }

    Ok(())
}

/// Dispatch a notification asynchronously via the job queue.
///
/// Pre-renders all channel payloads immediately, then dispatches a
/// `SendNotificationJob` to the worker. Returns immediately without
/// waiting for delivery.
///
/// ```ignore
/// app.notify_queued(&user, &OrderShipped { order_id: "123".into() }).await?;
/// ```
/// Pre-render all notification payloads and wrap in a job for async dispatch.
pub fn build_notification_job(
    notifiable: &dyn Notifiable,
    notification: &dyn Notification,
) -> SendNotificationJob {
    let channels = notification.via();
    let email_payload = notification.to_email(notifiable);
    let database_payload = notification.to_database();
    let broadcast_payload = notification.to_broadcast();

    let mut custom_payloads = Vec::new();
    for channel_id in &channels {
        if *channel_id != NOTIFY_EMAIL
            && *channel_id != NOTIFY_DATABASE
            && *channel_id != NOTIFY_BROADCAST
        {
            if let Some(data) = notification.to_channel(channel_id.as_ref(), notifiable) {
                custom_payloads.push((channel_id.clone(), data));
            }
        }
    }

    SendNotificationJob {
        notifiable_id: notifiable.notification_id(),
        notification_type: notification.notification_type().to_string(),
        channels,
        email_payload,
        database_payload,
        broadcast_payload,
        custom_payloads,
    }
}

pub async fn notify_queued(
    app: &AppContext,
    notifiable: &dyn Notifiable,
    notification: &dyn Notification,
) -> Result<()> {
    let job = build_notification_job(notifiable, notification);
    app.jobs()?.dispatch(job).await
}
