pub(crate) mod callback;
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
    let channels = callback::notification_channels(notification)?;
    let notification_type = callback::notification_type(notification)?;

    for channel_id in channels {
        if let Some(channel) = registry.get(&channel_id) {
            if let Err(error) = callback::send_channel_adapter(
                &channel_id,
                channel.as_ref(),
                app,
                notifiable,
                notification,
            )
            .await
            {
                tracing::error!(
                    channel = %channel_id,
                    notification_type = %notification_type,
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
    match try_build_notification_job(notifiable, notification) {
        Ok(job) => job,
        Err(error) => {
            tracing::error!(
                error = %error,
                "notification job payload rendering failed"
            );
            SendNotificationJob {
                notifiable_id: String::new(),
                notification_type: "unknown".to_string(),
                channels: Vec::new(),
                email_payload: None,
                database_payload: None,
                broadcast_payload: None,
                custom_payloads: Vec::new(),
            }
        }
    }
}

pub(crate) fn try_build_notification_job(
    notifiable: &dyn Notifiable,
    notification: &dyn Notification,
) -> Result<SendNotificationJob> {
    let channels = callback::notification_channels(notification)?;
    let email_payload = callback::notification_email(notification, notifiable)?;
    let database_payload = callback::notification_database(notification)?;
    let broadcast_payload = callback::notification_broadcast(notification)?;

    let mut custom_payloads = Vec::new();
    for channel_id in &channels {
        if *channel_id != NOTIFY_EMAIL
            && *channel_id != NOTIFY_DATABASE
            && *channel_id != NOTIFY_BROADCAST
        {
            if let Some(data) = callback::notification_channel_payload(
                notification,
                channel_id.as_ref(),
                notifiable,
            )? {
                custom_payloads.push((channel_id.clone(), data));
            }
        }
    }

    Ok(SendNotificationJob {
        notifiable_id: callback::notifiable_id(notifiable)?,
        notification_type: callback::notification_type(notification)?,
        channels,
        email_payload,
        database_payload,
        broadcast_payload,
        custom_payloads,
    })
}

pub async fn notify_queued(
    app: &AppContext,
    notifiable: &dyn Notifiable,
    notification: &dyn Notification,
) -> Result<()> {
    let job = try_build_notification_job(notifiable, notification)?;
    app.jobs()?.dispatch(job).await
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::config::ConfigRepository;
    use crate::foundation::Container;
    use crate::validation::RuleRegistry;

    fn test_app(
        channels: Vec<(NotificationChannelId, Arc<dyn NotificationChannel>)>,
    ) -> AppContext {
        let container = Container::new();
        let handle = NotificationChannelRegistryBuilder::shared();
        {
            let mut builder = handle.lock().expect("notification registry lock");
            for (id, channel) in channels {
                builder.register(id, channel).unwrap();
            }
        }
        let registry = Arc::new(NotificationChannelRegistryBuilder::freeze_shared(handle));
        container.singleton_arc(registry).unwrap();
        AppContext::new(container, ConfigRepository::empty(), RuleRegistry::new()).unwrap()
    }

    struct TestNotifiable;

    impl Notifiable for TestNotifiable {
        fn notification_id(&self) -> String {
            "user-1".to_string()
        }

        fn route_notification_for(&self, channel: &str) -> Option<String> {
            (channel == "email").then_some("user@example.com".to_string())
        }
    }

    struct TestNotification {
        channels: Vec<NotificationChannelId>,
    }

    impl TestNotification {
        fn new(channels: Vec<NotificationChannelId>) -> Self {
            Self { channels }
        }
    }

    impl Notification for TestNotification {
        fn notification_type(&self) -> &str {
            "test.notification"
        }

        fn via(&self) -> Vec<NotificationChannelId> {
            self.channels.clone()
        }

        fn to_database(&self) -> Option<serde_json::Value> {
            Some(json!({ "ok": true }))
        }

        fn to_channel(
            &self,
            channel: &str,
            _notifiable: &dyn Notifiable,
        ) -> Option<serde_json::Value> {
            Some(json!({ "channel": channel }))
        }
    }

    struct PanickingViaNotification;

    impl Notification for PanickingViaNotification {
        fn notification_type(&self) -> &str {
            "panic.via"
        }

        fn via(&self) -> Vec<NotificationChannelId> {
            panic!("via exploded")
        }
    }

    struct PanickingDatabaseNotification;

    impl Notification for PanickingDatabaseNotification {
        fn notification_type(&self) -> &str {
            "panic.database"
        }

        fn via(&self) -> Vec<NotificationChannelId> {
            vec![NOTIFY_DATABASE]
        }

        fn to_database(&self) -> Option<serde_json::Value> {
            panic!("database renderer exploded")
        }
    }

    struct PanickingCustomPayloadNotification;

    impl Notification for PanickingCustomPayloadNotification {
        fn notification_type(&self) -> &str {
            "panic.custom"
        }

        fn via(&self) -> Vec<NotificationChannelId> {
            vec![NotificationChannelId::new("sms")]
        }

        fn to_channel(
            &self,
            _channel: &str,
            _notifiable: &dyn Notifiable,
        ) -> Option<serde_json::Value> {
            panic!("custom renderer exploded")
        }
    }

    struct PanickingChannel;

    #[async_trait]
    impl NotificationChannel for PanickingChannel {
        async fn send(
            &self,
            _app: &AppContext,
            _notifiable: &dyn Notifiable,
            _notification: &dyn Notification,
        ) -> Result<()> {
            panic!("channel exploded")
        }
    }

    struct RecordingChannel {
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl NotificationChannel for RecordingChannel {
        async fn send(
            &self,
            _app: &AppContext,
            _notifiable: &dyn Notifiable,
            _notification: &dyn Notification,
        ) -> Result<()> {
            self.log.lock().unwrap().push("sent".to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn notification_channel_panic_isolated_and_later_channels_continue() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let panic_id = NotificationChannelId::new("panic");
        let ok_id = NotificationChannelId::new("ok");
        let app = test_app(vec![
            (panic_id.clone(), Arc::new(PanickingChannel)),
            (
                ok_id.clone(),
                Arc::new(RecordingChannel { log: log.clone() }),
            ),
        ]);
        let notification = TestNotification::new(vec![panic_id, ok_id]);

        notify(&app, &TestNotifiable, &notification).await.unwrap();

        assert_eq!(log.lock().unwrap().as_slice(), ["sent"]);
    }

    #[tokio::test]
    async fn notification_via_panic_becomes_error() {
        let app = test_app(Vec::new());

        let error = notify(&app, &TestNotifiable, &PanickingViaNotification)
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("notification via callback panicked: via exploded"));
    }

    #[tokio::test]
    async fn built_in_channel_renderer_panic_becomes_error() {
        let app = test_app(Vec::new());

        let error = DatabaseNotificationChannel
            .send(&app, &TestNotifiable, &PanickingDatabaseNotification)
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("notification database renderer panicked: database renderer exploded"));
    }

    #[tokio::test]
    async fn notify_queued_renderer_panic_becomes_error_before_dispatch() {
        let app = test_app(Vec::new());

        let error = notify_queued(&app, &TestNotifiable, &PanickingCustomPayloadNotification)
            .await
            .unwrap_err();

        assert!(error.to_string().contains(
            "notification custom channel `sms` renderer panicked: custom renderer exploded"
        ));
    }

    #[test]
    fn public_notification_job_builder_does_not_panic_on_renderer_panic() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            build_notification_job(&TestNotifiable, &PanickingCustomPayloadNotification)
        }));

        let job = result.expect("builder should isolate notification renderer panics");
        assert!(job.channels.is_empty());
        assert_eq!(job.notification_type, "unknown");
    }
}
