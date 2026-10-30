//! Scheduled messages.

mod common;

use std::time::Duration;

use common::Harness;
use serde_json::{Value, json};
use slack_emu_core::now;

async fn general(h: &Harness, who: &str) -> String {
    let r = h.ok(who, "users.conversations", json!({})).await;
    r["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "general")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Waits for a message with this text in the channel.
async fn wait_for_text(h: &Harness, who: &str, channel: &str, text: &str) -> Value {
    for _ in 0..50 {
        let history = h.ok(who, "conversations.history", json!({"channel": channel})).await;
        if let Some(m) = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["text"] == text)
        {
            return m.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{text:?} never posted");
}

#[tokio::test]
async fn messages_post_when_due() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let (bot, bot_user) = h.bot("reminder").await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;

    h.err(
        &bot,
        "chat.scheduleMessage",
        json!({"channel": general, "text": "late", "post_at": now() - 5}),
        "time_in_past",
    )
    .await;
    h.err(
        &bot,
        "chat.scheduleMessage",
        json!({"channel": general, "text": "far", "post_at": now() + 121 * 24 * 3600}),
        "time_too_far",
    )
    .await;

    let post_at = now() + 1;
    let r = h
        .ok(
            &bot,
            "chat.scheduleMessage",
            json!({"channel": general, "text": "Standup time!", "post_at": post_at}),
        )
        .await;
    let id = r["scheduled_message_id"].as_str().unwrap().to_owned();
    assert!(id.starts_with('Q'));
    assert_eq!(r["post_at"], post_at);
    assert_eq!(r["message"]["type"], "delayed_message");

    // Only the scheduler sees it listed.
    let listed = h
        .ok(&bot, "chat.scheduledMessages.list", json!({"channel": general}))
        .await;
    assert_eq!(listed["scheduled_messages"][0]["id"], id.as_str());
    assert_eq!(listed["scheduled_messages"][0]["text"], "Standup time!");
    let theirs = h.ok(&alice, "chat.scheduledMessages.list", json!({})).await;
    assert_eq!(theirs["scheduled_messages"], json!([]));

    let posted = wait_for_text(&h, &alice, &general, "Standup time!").await;
    assert_eq!(posted["user"], bot_user.as_str());
    assert!(posted["bot_id"].is_string());
    assert_eq!(
        h.ok(&bot, "chat.scheduledMessages.list", json!({})).await["scheduled_messages"],
        json!([])
    );
}

#[tokio::test]
async fn people_schedule_and_cancel() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let r = h
        .ok(
            &alice,
            "chat.scheduleMessage",
            json!({"channel": general, "text": "later", "post_at": now() + 3600}),
        )
        .await;
    let id = r["scheduled_message_id"].clone();
    let bob = h.signup("bob@example.com").await;
    h.err(
        &bob,
        "chat.deleteScheduledMessage",
        json!({"channel": general, "scheduled_message_id": id}),
        "invalid_scheduled_message_id",
    )
    .await;
    h.ok(
        &alice,
        "chat.deleteScheduledMessage",
        json!({"channel": general, "scheduled_message_id": id}),
    )
    .await;
    assert_eq!(
        h.ok(&alice, "chat.scheduledMessages.list", json!({})).await["scheduled_messages"],
        json!([])
    );
}

#[tokio::test]
async fn revoked_tokens_dont_post() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let (bot, bot_user) = h.bot("quitter").await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;
    h.ok(
        &bot,
        "chat.scheduleMessage",
        json!({"channel": general, "text": "ghost", "post_at": now() + 1}),
    )
    .await;
    h.ok(&bot, "auth.revoke", json!({})).await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    slack_emu_server::scheduler::post_due(&h.state).await.unwrap();
    let history = h.ok(&alice, "conversations.history", json!({"channel": general})).await;
    assert!(
        !history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "ghost")
    );
}
