//! User groups.

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
async fn groups_members_and_mentions() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let bob_id = h.user_id(&bob).await;
    let alice_id = h.user_id(&alice).await;
    let general = general(&h, &alice).await;
    let (bot, _) = h.bot("admin-bot").await;

    let mut bus = h.state.bus.subscribe();
    let created = h
        .ok(
            &bot,
            "usergroups.create",
            json!({"name": "Platform Oncall", "description": "Pager rotation"}),
        )
        .await;
    let g = &created["usergroup"];
    let id = g["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with('S'));
    assert_eq!(g["handle"], "platform-oncall");
    assert_eq!(bus.recv().await.unwrap().event["type"], "subteam_created");

    h.err(
        &bot,
        "usergroups.create",
        json!({"name": "Platform Oncall", "handle": "x"}),
        "name_already_exists",
    )
    .await;
    h.err(
        &bot,
        "usergroups.create",
        json!({"name": "Other", "handle": "platform-oncall"}),
        "handle_already_exists",
    )
    .await;
    // Handles can't collide with people's.
    h.err(
        &bot,
        "usergroups.create",
        json!({"name": "Bobs", "handle": "bob"}),
        "handle_already_exists",
    )
    .await;
    h.err(
        &bot,
        "usergroups.create",
        json!({"name": "All", "handle": "here"}),
        "invalid_handle",
    )
    .await;

    let updated = h
        .ok(
            &bot,
            "usergroups.users.update",
            json!({"usergroup": id, "users": format!("{bob_id},{alice_id}")}),
        )
        .await;
    assert_eq!(updated["usergroup"]["user_count"], 2);
    let changed = loop {
        let ev = bus.recv().await.unwrap();
        if ev.event["type"] == "subteam_members_changed" {
            break ev.event.clone();
        }
    };
    assert_eq!(changed["added_users_count"], "2");
    h.err(
        &bot,
        "usergroups.users.update",
        json!({"usergroup": id, "users": "UNOBODY"}),
        "invalid_users",
    )
    .await;

    // `@platform-oncall` becomes a group mention, which counts for members.
    let posted = h
        .ok(
            &alice,
            "chat.postMessage",
            json!({"channel": general, "text": "@platform-oncall the site is down", "link_names": true}),
        )
        .await;
    assert_eq!(
        posted["message"]["text"],
        format!("<!subteam^{id}|@platform-oncall> the site is down")
    );
    let counts = h.ok(&bob, "client.counts", json!({})).await;
    let mine = counts["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == general.as_str())
        .unwrap();
    assert_eq!(mine["mention_count"], 1);

    // Disabled groups drop out of listings unless asked for.
    h.ok(&bot, "usergroups.disable", json!({"usergroup": id})).await;
    h.err(&bot, "usergroups.disable", json!({"usergroup": id}), "already_disabled")
        .await;
    assert_eq!(h.ok(&bot, "usergroups.list", json!({})).await["usergroups"], json!([]));
    let all = h
        .ok(
            &bot,
            "usergroups.list",
            json!({"include_disabled": true, "include_users": true}),
        )
        .await;
    assert!(all["usergroups"][0]["date_delete"].as_i64().unwrap() > 0);
    assert_eq!(all["usergroups"][0]["users"].as_array().unwrap().len(), 2);
    h.ok(&bot, "usergroups.enable", json!({"usergroup": id})).await;
    let renamed = h
        .ok(
            &bot,
            "usergroups.update",
            json!({"usergroup": id, "name": "SRE", "handle": "sre"}),
        )
        .await;
    assert_eq!(renamed["usergroup"]["handle"], "sre");
    let members = h.ok(&bot, "usergroups.users.list", json!({"usergroup": id})).await;
    assert_eq!(members["users"].as_array().unwrap().len(), 2);
}
