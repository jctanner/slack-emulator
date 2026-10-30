//! Pins and bookmarks.

mod common;

use common::Harness;
use serde_json::json;

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

#[tokio::test]
async fn pinning_messages() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let bob = h.signup("bob@example.com").await;
    let general = general(&h, &alice).await;
    let (bot, bot_user) = h.bot("pinner").await;
    let posted = h
        .ok(
            &alice,
            "chat.postMessage",
            json!({"channel": general, "text": "Release notes"}),
        )
        .await;
    let ts = posted["ts"].as_str().unwrap().to_owned();
    let pin = json!({"channel": general, "timestamp": ts});

    // Bots must be in the channel.
    h.err(&bot, "pins.add", pin.clone(), "not_in_channel").await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;

    let mut bus = h.state.bus.subscribe();
    h.ok(&bob, "pins.add", pin.clone()).await;
    h.err(&bob, "pins.add", pin.clone(), "already_pinned").await;
    let ev = loop {
        let ev = bus.recv().await.unwrap();
        if ev.event["type"] == "pin_added" {
            break ev.event.clone();
        }
    };
    assert_eq!(ev["channel_id"], general.as_str());
    assert_eq!(ev["item"]["message"]["text"], "Release notes");
    assert_eq!(ev["item_user"], alice_id.as_str());
    assert_eq!(ev["pin_count"], 1);

    // The message shows where it's pinned, and the channel notes the pin.
    let history = h
        .ok(&bot, "conversations.history", json!({"channel": general, "limit": 5}))
        .await;
    assert_eq!(history["messages"][0]["subtype"], "pinned_item");
    let pinned = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["ts"] == ts.as_str())
        .unwrap();
    assert_eq!(pinned["pinned_to"], json!([general]));
    let listed = h.ok(&bot, "pins.list", json!({"channel": general})).await;
    assert_eq!(listed["items"][0]["message"]["ts"], ts.as_str());
    assert_eq!(listed["items"][0]["created_by"], h.user_id(&bob).await.as_str());

    // System messages can't be pinned.
    let join_ts = history["messages"][0]["ts"].clone();
    h.err(
        &alice,
        "pins.add",
        json!({"channel": general, "timestamp": join_ts}),
        "not_pinnable",
    )
    .await;

    h.ok(&bot, "pins.remove", pin.clone()).await;
    h.err(&bot, "pins.remove", pin.clone(), "no_pin").await;
    assert_eq!(
        h.ok(&bot, "pins.list", json!({"channel": general})).await["items"],
        json!([])
    );
}

#[tokio::test]
async fn channel_bookmarks() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let (bot, bot_user) = h.bot("linker").await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;

    let added = h
        .ok(
            &bot,
            "bookmarks.add",
            json!({"channel_id": general, "title": "Runbook", "type": "link",
                   "link": "https://example.com/runbook", "emoji": ":book:"}),
        )
        .await;
    let b = &added["bookmark"];
    assert!(b["id"].as_str().unwrap().starts_with("Bk"));
    assert_eq!(b["type"], "link");
    assert!(b["app_id"].is_string(), "bookmarks remember the app that added them");

    h.err(
        &alice,
        "bookmarks.add",
        json!({"channel_id": general, "title": "x", "type": "link", "link": "javascript:alert(1)"}),
        "invalid_link",
    )
    .await;
    h.err(
        &alice,
        "bookmarks.add",
        json!({"channel_id": general, "title": "x", "type": "folder", "link": "https://x.test"}),
        "invalid_bookmark_type",
    )
    .await;

    let edited = h
        .ok(
            &alice,
            "bookmarks.edit",
            json!({"channel_id": general, "bookmark_id": b["id"], "title": "Runbook v2", "emoji": ""}),
        )
        .await;
    assert_eq!(edited["bookmark"]["title"], "Runbook v2");
    assert_eq!(edited["bookmark"]["emoji"], json!(null));
    assert_eq!(
        edited["bookmark"]["last_updated_by_user_id"],
        h.user_id(&alice).await.as_str()
    );

    let listed = h.ok(&bot, "bookmarks.list", json!({"channel_id": general})).await;
    assert_eq!(listed["bookmarks"].as_array().unwrap().len(), 1);
    h.ok(
        &alice,
        "bookmarks.remove",
        json!({"channel_id": general, "bookmark_id": b["id"]}),
    )
    .await;
    h.err(
        &alice,
        "bookmarks.remove",
        json!({"channel_id": general, "bookmark_id": b["id"]}),
        "not_found",
    )
    .await;
    assert_eq!(
        h.ok(&bot, "bookmarks.list", json!({"channel_id": general})).await["bookmarks"],
        json!([])
    );
}
