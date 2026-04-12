use async_trait::async_trait;

use crate::database::DbValue;
use crate::foundation::{AppContext, Result};

use super::{Notifiable, Notification};

/// Adapter trait for notification delivery channels.
///
/// Framework provides built-in channels (email, database, broadcast).
/// Projects can register custom channels via `register_notification_channel()`.
#[async_trait]
pub trait NotificationChannel: Send + Sync + 'static {
    async fn send(
        &self,
        app: &AppContext,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
    ) -> Result<()>;
}

/// Built-in email notification channel.
pub struct EmailNotificationChannel;

#[async_trait]
impl NotificationChannel for EmailNotificationChannel {
    async fn send(
        &self,
        app: &AppContext,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
    ) -> Result<()> {
        let Some(_email) = notifiable.route_notification_for("email") else {
            return Ok(());
        };
        let Some(message) = notification.to_email(notifiable) else {
            return Ok(());
        };
        app.email()?.send(message).await
    }
}

/// Built-in database notification channel.
/// Stores notifications in the `notifications` table.
pub struct DatabaseNotificationChannel;

#[async_trait]
impl NotificationChannel for DatabaseNotificationChannel {
    async fn send(
        &self,
        app: &AppContext,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
    ) -> Result<()> {
        let Some(data) = notification.to_database() else {
            return Ok(());
        };
        let db = app.database()?;
        db.raw_execute(
            "INSERT INTO notifications (notifiable_id, type, data, created_at) VALUES ($1, $2, $3, NOW())",
            &[
                DbValue::Text(notifiable.notification_id()),
                DbValue::Text(notification.notification_type().to_string()),
                DbValue::Json(data),
            ],
        )
        .await?;
        Ok(())
    }
}

/// Built-in WebSocket broadcast notification channel.
pub struct BroadcastNotificationChannel;

#[async_trait]
impl NotificationChannel for BroadcastNotificationChannel {
    async fn send(
        &self,
        app: &AppContext,
        notifiable: &dyn Notifiable,
        notification: &dyn Notification,
    ) -> Result<()> {
        let Some(payload) = notification.to_broadcast() else {
            return Ok(());
        };
        let ws = app.websocket()?;
        let channel_id = crate::support::ChannelId::owned(format!(
            "notifications:{}",
            notifiable.notification_id()
        ));
        let event = crate::support::ChannelEventId::new("notification");
        ws.publish(channel_id, event, None::<&str>, payload).await
    }
}
