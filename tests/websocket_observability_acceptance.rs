use axum::http::StatusCode;
use forge::support::{ChannelId, GuardId, PermissionId};
use forge::testing::TestApp;
use forge::websocket::WebSocketChannelOptions;
use serde_json::Value;

#[tokio::test]
async fn ws_presence_endpoint_returns_members_for_presence_channel() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel_with_options(
                ChannelId::new("team"),
                |_ctx, _payload| async { Ok(()) },
                WebSocketChannelOptions::new().presence(true),
            )?;
            r.channel(ChannelId::new("public"), |_ctx, _payload| async { Ok(()) })?;
            Ok(())
        })
        .build()
        .await;

    app.seed_presence(&ChannelId::new("team"), "user_1", 1_713_000_000)
        .await
        .unwrap();

    let response = app.client().get("/_forge/ws/presence/team").send().await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json();
    assert_eq!(body["channel"], "team");
    assert_eq!(body["count"], 1);
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["actor_id"], "user_1");
    assert_eq!(members[0]["joined_at"], 1_713_000_000);
}

#[tokio::test]
async fn ws_presence_endpoint_returns_404_for_non_presence_channel() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("public"), |_ctx, _payload| async { Ok(()) })?;
            Ok(())
        })
        .build()
        .await;

    let response = app.client().get("/_forge/ws/presence/public").send().await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn ws_presence_endpoint_returns_404_for_unregistered_channel() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|_r| Ok(()))
        .build()
        .await;

    let response = app.client().get("/_forge/ws/presence/ghost").send().await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn ws_channels_endpoint_lists_registered_channels() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel_with_options(
                ChannelId::new("chat"),
                |_ctx, _payload| async { Ok(()) },
                WebSocketChannelOptions::new()
                    .presence(true)
                    .replay(10)
                    .allow_client_events(false)
                    .guard(GuardId::new("api"))
                    .permissions([PermissionId::new("chat:read")]),
            )?;
            r.channel(ChannelId::new("public"), |_ctx, _payload| async { Ok(()) })?;
            Ok(())
        })
        .build()
        .await;

    let response = app.client().get("/_forge/ws/channels").send().await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json();
    let channels = body["channels"].as_array().expect("channels array");
    assert_eq!(channels.len(), 2);

    let chat = channels
        .iter()
        .find(|c| c["id"] == "chat")
        .expect("chat present");
    assert_eq!(chat["presence"], Value::Bool(true));
    assert_eq!(chat["replay_count"], 10);
    assert_eq!(chat["allow_client_events"], Value::Bool(false));
    assert_eq!(chat["requires_auth"], Value::Bool(true));
    assert_eq!(chat["guard"], "api");
    assert_eq!(chat["permissions"], Value::Array(vec!["chat:read".into()]));

    let public = channels
        .iter()
        .find(|c| c["id"] == "public")
        .expect("public present");
    assert_eq!(public["presence"], Value::Bool(false));
    assert_eq!(public["requires_auth"], Value::Bool(false));
}

#[tokio::test]
async fn ws_history_redacts_payloads_by_default() {
    use forge::support::ChannelEventId;

    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("history-redact"), |_ctx, _payload| async {
                Ok(())
            })?;
            Ok(())
        })
        .build()
        .await;

    let publisher = app.app().websocket().unwrap();
    publisher
        .publish(
            ChannelId::new("history-redact"),
            ChannelEventId::new("created"),
            None,
            serde_json::json!({ "secret": "hello world" }),
        )
        .await
        .unwrap();

    let response = app
        .client()
        .get("/_forge/ws/history/history-redact")
        .send()
        .await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message["channel"], "history-redact");
    assert_eq!(message["event"], "created");
    assert!(
        message.get("payload").is_none(),
        "payload must be redacted by default"
    );
    assert!(message["payload_size_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn ws_history_returns_payloads_when_flag_is_set() {
    use forge::support::ChannelEventId;

    // Write a temp config dir with include_payloads = true.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("00-observability.toml"),
        r#"
[observability.websocket]
include_payloads = true
"#,
    )
    .unwrap();

    let app = TestApp::builder()
        .load_config_dir(tmp.path())
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("history-full"), |_ctx, _payload| async {
                Ok(())
            })?;
            Ok(())
        })
        .build()
        .await;

    let publisher = app.app().websocket().unwrap();
    publisher
        .publish(
            ChannelId::new("history-full"),
            ChannelEventId::new("created"),
            None,
            serde_json::json!({ "secret": "hello world" }),
        )
        .await
        .unwrap();

    let response = app
        .client()
        .get("/_forge/ws/history/history-full")
        .send()
        .await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["payload"]["secret"], "hello world");
}

#[tokio::test]
async fn ws_history_returns_404_for_unregistered_channel() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|_r| Ok(()))
        .build()
        .await;

    let response = app.client().get("/_forge/ws/history/ghost").send().await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn ws_history_clamps_limit_to_buffer_size() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("events"), |_ctx, _payload| async { Ok(()) })?;
            Ok(())
        })
        .build()
        .await;

    let response = app
        .client()
        .get("/_forge/ws/history/events?limit=999")
        .send()
        .await;
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn ws_stats_exposes_global_and_per_channel_counters() {
    let app = TestApp::builder()
        .enable_observability()
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("alpha"), |_ctx, _payload| async { Ok(()) })?;
            r.channel(ChannelId::new("idle"), |_ctx, _payload| async { Ok(()) })?;
            Ok(())
        })
        .build()
        .await;

    // Drive traffic via the diagnostics API directly.
    let diagnostics = app.app().diagnostics().unwrap();
    diagnostics.record_websocket_subscription_opened_on(&ChannelId::new("alpha"));
    diagnostics.record_websocket_inbound_message_on(&ChannelId::new("alpha"));
    diagnostics.record_websocket_outbound_message_on(&ChannelId::new("alpha"));
    diagnostics.record_websocket_outbound_message_on(&ChannelId::new("alpha"));

    let response = app.client().get("/_forge/ws/stats").send().await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json();

    assert_eq!(body["global"]["active_subscriptions"], 1);
    assert_eq!(body["global"]["inbound_messages_total"], 1);
    assert_eq!(body["global"]["outbound_messages_total"], 2);

    let channels = body["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 2, "registered-but-idle channels appear too");

    let alpha = channels.iter().find(|c| c["id"] == "alpha").unwrap();
    assert_eq!(alpha["subscriptions_total"], 1);
    assert_eq!(alpha["active_subscriptions"], 1);
    assert_eq!(alpha["inbound_messages_total"], 1);
    assert_eq!(alpha["outbound_messages_total"], 2);

    let idle = channels.iter().find(|c| c["id"] == "idle").unwrap();
    assert_eq!(idle["subscriptions_total"], 0);
    assert_eq!(idle["outbound_messages_total"], 0);
}

#[tokio::test]
async fn publish_sets_history_ttl_by_default() {
    use forge::support::ChannelEventId;

    let app = TestApp::builder()
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("ttl-default"), |_ctx, _payload| async {
                Ok(())
            })?;
            Ok(())
        })
        .build()
        .await;

    assert_eq!(
        app.history_ttl(&ChannelId::new("ttl-default"))
            .await
            .unwrap(),
        None,
        "no TTL before first publish",
    );

    app.app()
        .websocket()
        .unwrap()
        .publish(
            ChannelId::new("ttl-default"),
            ChannelEventId::new("created"),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    assert_eq!(
        app.history_ttl(&ChannelId::new("ttl-default"))
            .await
            .unwrap(),
        Some(604_800),
        "publish applies the default 7-day history TTL",
    );
}

#[tokio::test]
async fn publish_skips_ttl_when_configured_to_zero() {
    use forge::support::ChannelEventId;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("00-websocket.toml"),
        r#"
[websocket]
history_ttl_seconds = 0
"#,
    )
    .unwrap();

    let app = TestApp::builder()
        .load_config_dir(tmp.path())
        .register_websocket_routes(|r| {
            r.channel(ChannelId::new("ttl-disabled"), |_ctx, _payload| async {
                Ok(())
            })?;
            Ok(())
        })
        .build()
        .await;

    app.app()
        .websocket()
        .unwrap()
        .publish(
            ChannelId::new("ttl-disabled"),
            ChannelEventId::new("created"),
            None,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    assert_eq!(
        app.history_ttl(&ChannelId::new("ttl-disabled"))
            .await
            .unwrap(),
        None,
        "history_ttl_seconds = 0 disables expire()",
    );
}
