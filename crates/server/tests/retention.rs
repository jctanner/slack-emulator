//! Message retention.

mod common;

use common::Harness;
use serde_json::json;
use slack_emu_core::now;
use slack_emu_server::retention::sweep;

async fn channel(h: &Harness, who: &str, name: &str) -> String {
    let r = h.ok(who, "users.conversations", json!({})).await;
    r["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn policies_prune_old_messages() {
    let h = Harness::new().await;
    // The first account owns the workspace.
    let owner = h.signup("owner@example.com").await;
    let member = h.signup("member@example.com").await;
    let general = channel(&h, &owner, "general").await;
    h.ok(&owner, "conversations.join", json!({"channel": "random"})).await;
    let random = channel(&h, &owner, "random").await;

    h.err(&member, "emu.retention.set", json!({"days": 30}), "not_an_admin")
        .await;
    h.err(
        &member,
        "admin.conversations.setCustomRetention",
        json!({"channel_id": random, "duration_days": 1}),
        "not_an_admin",
    )
    .await;
    h.err(
        &owner,
        "admin.conversations.setCustomRetention",
        json!({"channel_id": random, "duration_days": 0}),
        "invalid_arguments",
    )
    .await;

    h.ok(&owner, "emu.retention.set", json!({"days": 30})).await;
    h.ok(
        &owner,
        "admin.conversations.setCustomRetention",
        json!({"channel_id": random, "duration_days": 1}),
    )
    .await;
    let got = h
        .ok(
            &owner,
            "admin.conversations.getCustomRetention",
            json!({"channel_id": random}),
        )
        .await;
    assert_eq!(
        (got["is_policy_enabled"].clone(), got["duration_days"].clone()),
        (json!(true), json!(1))
    );
    let all = h.ok(&member, "emu.retention.get", json!({})).await;
    assert_eq!(all["workspace_days"], 30);
    assert_eq!(all["channels"][0]["channel_id"], random.as_str());

    h.ok(
        &owner,
        "chat.postMessage",
        json!({"channel": general, "text": "keep for a month"}),
    )
    .await;
    h.ok(
        &owner,
        "chat.postMessage",
        json!({"channel": random, "text": "gone tomorrow"}),
    )
    .await;

    // Two days on, #random's one-day policy applies; #general keeps 30 days.
    let mut bus = h.state.bus.subscribe();
    assert!(sweep(&h.state, now() + 2 * 86_400).await.unwrap() >= 1);
    let ev = bus.recv().await.unwrap();
    assert_eq!(ev.event["type"], "emu_history_pruned");
    assert_eq!(ev.event["channel"], random.as_str());
    let texts = |r: serde_json::Value| -> Vec<String> {
        r["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(texts(h.ok(&owner, "conversations.history", json!({"channel": random})).await).is_empty());
    assert!(
        texts(h.ok(&owner, "conversations.history", json!({"channel": general})).await)
            .contains(&"keep for a month".to_owned())
    );

    // A month on, the workspace default takes #general's too.
    sweep(&h.state, now() + 31 * 86_400).await.unwrap();
    assert!(texts(h.ok(&owner, "conversations.history", json!({"channel": general})).await).is_empty());

    // Removing policies keeps messages forever again.
    h.ok(
        &owner,
        "admin.conversations.removeCustomRetention",
        json!({"channel_id": random}),
    )
    .await;
    h.ok(&owner, "emu.retention.set", json!({})).await;
    assert_eq!(
        h.ok(&owner, "emu.retention.get", json!({})).await["workspace_days"],
        json!(null)
    );
    h.ok(
        &owner,
        "chat.postMessage",
        json!({"channel": general, "text": "forever"}),
    )
    .await;
    assert_eq!(sweep(&h.state, now() + 365 * 86_400).await.unwrap(), 0);
}
