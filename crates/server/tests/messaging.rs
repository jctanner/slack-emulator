//! Channels, DMs, messages, threads and reactions through the Web API.

mod common;

use common::Harness;
use serde_json::{Value, json};

fn names(list: &Value) -> Vec<String> {
    let mut v: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect();
    v.sort();
    v
}

fn texts(msgs: &Value) -> Vec<String> {
    msgs.as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_owned())
        .collect()
}

async fn channel_id(h: &Harness, who: &str, name: &str) -> String {
    let r = h
        .ok(
            who,
            "users.conversations",
            json!({"types": "public_channel,private_channel"}),
        )
        .await;
    r["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("{name} not found in {r}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn post(h: &Harness, who: &str, channel: &str, text: &str) -> String {
    h.ok(who, "chat.postMessage", json!({"channel": channel, "text": text}))
        .await["ts"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn new_members_land_in_default_channels() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let r = h.ok(&alice, "users.conversations", json!({})).await;
    assert_eq!(names(&r["channels"]), ["general", "random"]);
    let general = r["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "general")
        .unwrap();
    assert_eq!(general["is_general"], true);

    let hist = h
        .ok(&alice, "conversations.history", json!({"channel": general["id"]}))
        .await;
    let first = &hist["messages"][0];
    assert_eq!(first["subtype"], "channel_join");
    assert_eq!(
        first["text"],
        format!("<@{}> has joined the channel", h.user_id(&alice).await)
    );

    // New workspaces get their own defaults.
    h.post_json(
        "/auth/workspaces",
        Some(&alice),
        json!({"name": "Acme", "domain": "acme"}),
    )
    .await;
    let me = h.get("/auth/me", Some(&alice)).await.body;
    let acme = me["workspaces"][1]["team"]["id"].as_str().unwrap().to_owned();
    let req = axum::http::Request::post("/api/users.conversations")
        .header("cookie", &alice)
        .header("x-slack-emu-team", &acme)
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(names(&h.send(req).await.body["channels"]), ["general", "random"]);
}

#[tokio::test]
async fn channel_lifecycle() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let bob_id = h.user_id(&bob).await;

    h.err(
        &alice,
        "conversations.create",
        json!({"name": "Bad Name"}),
        "invalid_name_specials",
    )
    .await;
    h.err(&alice, "conversations.create", json!({"name": "general"}), "name_taken")
        .await;
    let public = h.ok(&alice, "conversations.create", json!({"name": "dev"})).await["channel"].clone();
    assert_eq!(public["is_member"], true);
    assert_eq!(public["creator"], h.user_id(&alice).await.as_str());
    let secret = h
        .ok(
            &alice,
            "conversations.create",
            json!({"name": "secret", "is_private": true}),
        )
        .await["channel"]
        .clone();
    assert_eq!(secret["is_private"], true);
    let (dev, sec) = (public["id"].as_str().unwrap(), secret["id"].as_str().unwrap());

    // Bob sees the public channel but not the private one.
    let r = h
        .ok(
            &bob,
            "conversations.list",
            json!({"types": "public_channel,private_channel"}),
        )
        .await;
    assert_eq!(names(&r["channels"]), ["dev", "general", "random"]);
    h.err(&bob, "conversations.info", json!({"channel": sec}), "channel_not_found")
        .await;
    h.err(&bob, "conversations.join", json!({"channel": sec}), "channel_not_found")
        .await;

    // Humans can read public channels without joining, but not post.
    h.ok(&bob, "conversations.history", json!({"channel": dev})).await;
    h.err(
        &bob,
        "chat.postMessage",
        json!({"channel": dev, "text": "hi"}),
        "not_in_channel",
    )
    .await;
    let r = h.ok(&bob, "conversations.join", json!({"channel": dev})).await;
    assert_eq!(r["channel"]["is_member"], true);
    let r = h.ok(&bob, "conversations.join", json!({"channel": dev})).await;
    assert_eq!(r["warning"], "already_in_channel");
    post(&h, &bob, dev, "hi").await;

    // Invite to private.
    h.ok(&alice, "conversations.invite", json!({"channel": sec, "users": bob_id}))
        .await;
    h.err(
        &alice,
        "conversations.invite",
        json!({"channel": sec, "users": bob_id}),
        "already_in_channel",
    )
    .await;
    let info = h
        .ok(
            &bob,
            "conversations.info",
            json!({"channel": sec, "include_num_members": true}),
        )
        .await;
    assert_eq!(info["channel"]["num_members"], 2);
    let hist = h.ok(&bob, "conversations.history", json!({"channel": sec})).await;
    assert_eq!(
        hist["messages"][0]["text"],
        format!(
            "<@{bob_id}> has joined the channel (invited by <@{}>)",
            h.user_id(&alice).await
        )
    );

    // Kick and leave.
    h.ok(&alice, "conversations.kick", json!({"channel": sec, "user": bob_id}))
        .await;
    h.err(
        &bob,
        "conversations.history",
        json!({"channel": sec}),
        "channel_not_found",
    )
    .await;
    h.ok(&bob, "conversations.leave", json!({"channel": dev})).await;
    let general = channel_id(&h, &bob, "general").await;
    h.err(
        &bob,
        "conversations.leave",
        json!({"channel": general}),
        "cant_leave_general",
    )
    .await;

    // Rename, topic, purpose.
    h.err(
        &alice,
        "conversations.rename",
        json!({"channel": dev, "name": "random"}),
        "name_taken",
    )
    .await;
    let r = h
        .ok(
            &alice,
            "conversations.rename",
            json!({"channel": dev, "name": "engineering"}),
        )
        .await;
    assert_eq!(r["channel"]["name"], "engineering");
    let r = h
        .ok(
            &alice,
            "conversations.setTopic",
            json!({"channel": dev, "topic": "ship it"}),
        )
        .await;
    assert_eq!(r["channel"]["topic"]["value"], "ship it");
    h.ok(
        &alice,
        "conversations.setPurpose",
        json!({"channel": dev, "purpose": "building"}),
    )
    .await;
    let hist = h.ok(&alice, "conversations.history", json!({"channel": dev})).await;
    let subtypes: Vec<&str> = hist["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["subtype"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(
        subtypes[..4],
        ["channel_purpose", "channel_topic", "channel_name", "channel_leave"]
    );
    assert_eq!(hist["messages"][1]["text"], "set the channel topic: ship it");

    // Archive.
    h.err(
        &alice,
        "conversations.archive",
        json!({"channel": general}),
        "cant_archive_general",
    )
    .await;
    h.ok(&alice, "conversations.archive", json!({"channel": dev})).await;
    h.err(
        &alice,
        "conversations.archive",
        json!({"channel": dev}),
        "already_archived",
    )
    .await;
    h.err(
        &alice,
        "chat.postMessage",
        json!({"channel": dev, "text": "x"}),
        "is_archived",
    )
    .await;
    let r = h
        .ok(&alice, "conversations.list", json!({"exclude_archived": true}))
        .await;
    assert_eq!(names(&r["channels"]), ["general", "random"]);
    h.ok(&alice, "conversations.unarchive", json!({"channel": dev})).await;
    post(&h, &alice, dev, "back").await;
}

#[tokio::test]
async fn leave_reports_not_in_channel() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let dev = h.ok(&alice, "conversations.create", json!({"name": "dev"})).await["channel"]["id"].clone();
    h.err(&bob, "conversations.leave", json!({"channel": dev}), "not_in_channel")
        .await;
    h.ok(&alice, "conversations.leave", json!({"channel": dev})).await;
}

#[tokio::test]
async fn messages_threads_and_pagination() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let dev = h.ok(&alice, "conversations.create", json!({"name": "dev"})).await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.ok(&bob, "conversations.join", json!({"channel": dev})).await;

    h.err(&alice, "chat.postMessage", json!({"channel": dev}), "no_text")
        .await;
    h.err(
        &alice,
        "chat.postMessage",
        json!({"channel": "C0NOPE00000", "text": "x"}),
        "channel_not_found",
    )
    .await;
    let mut ts = Vec::new();
    for i in 1..=5 {
        ts.push(post(&h, &alice, &dev, &format!("m{i}")).await);
    }
    // Posting by name works too.
    post(&h, &alice, "#dev", "m6").await;

    // Pagination, newest first.
    let p1 = h
        .ok(&alice, "conversations.history", json!({"channel": dev, "limit": 4}))
        .await;
    assert_eq!(texts(&p1["messages"]), ["m6", "m5", "m4", "m3"]);
    assert_eq!(p1["has_more"], true);
    let cursor = p1["response_metadata"]["next_cursor"].as_str().unwrap();
    let p2 = h
        .ok(
            &alice,
            "conversations.history",
            json!({"channel": dev, "limit": 4, "cursor": cursor}),
        )
        .await;
    assert_eq!(texts(&p2["messages"])[..2], ["m2".to_owned(), "m1".to_owned()]);
    let r = h
        .ok(
            &alice,
            "conversations.history",
            json!({"channel": dev, "oldest": ts[1], "latest": ts[3], "inclusive": true}),
        )
        .await;
    assert_eq!(texts(&r["messages"]), ["m4", "m3", "m2"]);

    // Edit and delete permissions.
    h.err(
        &bob,
        "chat.update",
        json!({"channel": dev, "ts": ts[0], "text": "hacked"}),
        "cant_update_message",
    )
    .await;
    let r = h
        .ok(
            &alice,
            "chat.update",
            json!({"channel": dev, "ts": ts[0], "text": "m1 edited"}),
        )
        .await;
    assert_eq!(r["message"]["edited"]["user"], h.user_id(&alice).await.as_str());
    h.err(
        &bob,
        "chat.delete",
        json!({"channel": dev, "ts": ts[1]}),
        "cant_delete_message",
    )
    .await;
    h.ok(&alice, "chat.delete", json!({"channel": dev, "ts": ts[1]})).await;
    h.err(
        &alice,
        "chat.delete",
        json!({"channel": dev, "ts": ts[1]}),
        "message_not_found",
    )
    .await;

    // Threads.
    let parent = &ts[2];
    let r1 = h
        .ok(
            &bob,
            "chat.postMessage",
            json!({"channel": dev, "text": "r1", "thread_ts": parent}),
        )
        .await;
    assert_eq!(r1["message"]["thread_ts"], parent.as_str());
    assert_eq!(r1["message"]["parent_user_id"], h.user_id(&alice).await.as_str());
    let r1_ts = r1["ts"].as_str().unwrap().to_owned();
    // Replying to a reply stays in the same thread.
    h.ok(
        &alice,
        "chat.postMessage",
        json!({"channel": dev, "text": "r2", "thread_ts": r1_ts}),
    )
    .await;
    h.ok(
        &alice,
        "chat.postMessage",
        json!({"channel": dev, "text": "r3", "thread_ts": parent, "reply_broadcast": true}),
    )
    .await;
    h.err(
        &alice,
        "chat.postMessage",
        json!({"channel": dev, "text": "x", "thread_ts": "1.000001"}),
        "thread_not_found",
    )
    .await;

    let replies = h
        .ok(&alice, "conversations.replies", json!({"channel": dev, "ts": parent}))
        .await;
    assert_eq!(texts(&replies["messages"]), ["m3", "r1", "r2", "r3"]);
    let hist = h
        .ok(&alice, "conversations.history", json!({"channel": dev, "limit": 3}))
        .await;
    // The broadcast shows in the channel; plain replies don't.
    assert_eq!(texts(&hist["messages"]), ["r3", "m6", "m5"]);
    assert_eq!(hist["messages"][0]["subtype"], "thread_broadcast");
    let parent_msg = h
        .ok(
            &alice,
            "conversations.history",
            json!({"channel": dev, "latest": parent, "inclusive": true, "limit": 1}),
        )
        .await["messages"][0]
        .clone();
    assert_eq!(parent_msg["reply_count"], 3);
    assert_eq!(parent_msg["reply_users_count"], 2);

    // Deleting a parent with replies leaves a tombstone, which goes away with the last reply.
    h.ok(&alice, "chat.delete", json!({"channel": dev, "ts": parent})).await;
    let replies = h
        .ok(&alice, "conversations.replies", json!({"channel": dev, "ts": parent}))
        .await;
    assert_eq!(replies["messages"][0]["subtype"], "tombstone");
    for m in &replies["messages"].as_array().unwrap()[1..] {
        let who = if m["text"] == "r1" { &bob } else { &alice };
        h.ok(who, "chat.delete", json!({"channel": dev, "ts": m["ts"]})).await;
    }
    h.err(
        &alice,
        "conversations.replies",
        json!({"channel": dev, "ts": parent}),
        "thread_not_found",
    )
    .await;

    // Permalinks resolve to the client.
    let link = h
        .ok(
            &alice,
            "chat.getPermalink",
            json!({"channel": dev, "message_ts": ts[0]}),
        )
        .await["permalink"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = link.strip_prefix("http://localhost:8080").unwrap();
    let r = h.get(path, None).await;
    assert!(r.location.unwrap().ends_with(&format!("/{dev}?ts={}", ts[0])));
}

#[tokio::test]
async fn reactions() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let general = channel_id(&h, &alice, "general").await;
    let ts = post(&h, &alice, &general, "react to me").await;
    let args = |name: &str| json!({"channel": general, "timestamp": ts, "name": name});

    h.ok(&alice, "reactions.add", args(":tada:")).await;
    h.ok(&bob, "reactions.add", args("tada")).await;
    h.ok(&bob, "reactions.add", args("+1")).await;
    h.err(&bob, "reactions.add", args("tada"), "already_reacted").await;
    h.err(&bob, "reactions.add", args("bad name!"), "invalid_name").await;
    let r = h
        .ok(&bob, "reactions.get", json!({"channel": general, "timestamp": ts}))
        .await;
    let rs = &r["message"]["reactions"];
    assert_eq!(rs[0]["name"], "tada");
    assert_eq!(rs[0]["count"], 2);
    assert_eq!(rs[1]["name"], "+1");

    h.ok(&bob, "reactions.remove", args("+1")).await;
    h.err(&bob, "reactions.remove", args("+1"), "no_reaction").await;
    let hist = h
        .ok(&alice, "conversations.history", json!({"channel": general, "limit": 1}))
        .await;
    assert_eq!(hist["messages"][0]["reactions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn direct_messages() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let carol = h.signup("carol@example.com").await;
    let (alice_id, bob_id, carol_id) = (h.user_id(&alice).await, h.user_id(&bob).await, h.user_id(&carol).await);

    let r = h.ok(&alice, "conversations.open", json!({"users": bob_id})).await;
    let dm = r["channel"]["id"].as_str().unwrap().to_owned();
    assert!(dm.starts_with('D'));
    assert_eq!(r["channel"]["user"], bob_id.as_str());
    let again = h.ok(&bob, "conversations.open", json!({"users": alice_id})).await;
    assert_eq!(again["channel"]["id"], dm.as_str());
    assert_eq!(again["already_open"], true);
    assert_eq!(again["channel"]["user"], alice_id.as_str());

    // Posting to a user ID goes to the DM.
    let r = h
        .ok(&alice, "chat.postMessage", json!({"channel": bob_id, "text": "psst"}))
        .await;
    assert_eq!(r["channel"], dm.as_str());
    h.err(
        &carol,
        "conversations.history",
        json!({"channel": dm}),
        "channel_not_found",
    )
    .await;

    let r = h
        .ok(
            &alice,
            "conversations.open",
            json!({"users": format!("{bob_id},{carol_id}")}),
        )
        .await;
    assert_eq!(r["channel"]["is_mpim"], true);
    let name = r["channel"]["name"].as_str().unwrap();
    assert!(name.starts_with("mpdm-") && name.ends_with("-1"), "{name}");
    for handle in ["alice", "bob", "carol"] {
        assert!(name.contains(handle), "{name}");
    }

    let r = h.ok(&bob, "users.conversations", json!({"types": "im,mpim"})).await;
    assert_eq!(r["channels"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unread_counts() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let general = channel_id(&h, &alice, "general").await;

    let counts = |c: Value| {
        c["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == general.as_str())
            .unwrap()
            .clone()
    };
    // Bob joining after alice doesn't make general unread for alice (system messages don't count).
    let before = counts(h.ok(&alice, "client.counts", json!({})).await);
    assert_eq!(before["unread_count"], 0);

    post(&h, &bob, &general, "hello").await;
    let last = post(&h, &bob, &general, &format!("hey <@{alice_id}>")).await;
    let c = counts(h.ok(&alice, "client.counts", json!({})).await);
    assert_eq!(c["unread_count"], 2);
    assert_eq!(c["mention_count"], 1);
    assert_eq!(c["has_unreads"], true);
    // Your own messages are read.
    assert_eq!(counts(h.ok(&bob, "client.counts", json!({})).await)["unread_count"], 0);

    h.ok(&alice, "conversations.mark", json!({"channel": general, "ts": last}))
        .await;
    let c = counts(h.ok(&alice, "client.counts", json!({})).await);
    assert_eq!(c["unread_count"], 0);
    assert_eq!(c["last_read"], last.as_str());
}

#[tokio::test]
async fn bots_must_be_members() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let (bot, bot_id) = h.bot("helper").await;
    let general = channel_id(&h, &alice, "general").await;

    h.err(
        &bot,
        "conversations.history",
        json!({"channel": general}),
        "not_in_channel",
    )
    .await;
    h.err(
        &bot,
        "chat.postMessage",
        json!({"channel": general, "text": "hi"}),
        "not_in_channel",
    )
    .await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_id}),
    )
    .await;
    let r = h
        .ok(
            &bot,
            "chat.postMessage",
            json!({"channel": general, "text": "hi", "icon_emoji": ":robot_face:"}),
        )
        .await;
    assert!(r["message"]["bot_id"].as_str().unwrap().starts_with('B'));
    assert_eq!(r["message"]["icons"]["emoji"], ":robot_face:");

    // link_names turns handles into mentions.
    let r = h
        .ok(
            &bot,
            "chat.postMessage",
            json!({"channel": general, "text": "ping @alice in #random", "link_names": true}),
        )
        .await;
    let random = channel_id(&h, &alice, "random").await;
    assert_eq!(
        r["message"]["text"],
        format!("ping <@{}> in <#{random}|random>", h.user_id(&alice).await)
    );

    let users = h.ok(&bot, "users.list", json!({"limit": 1})).await;
    assert_eq!(users["members"].as_array().unwrap().len(), 1);
    let next = users["response_metadata"]["next_cursor"].as_str().unwrap();
    let more = h.ok(&bot, "users.list", json!({"cursor": next})).await;
    assert_eq!(more["members"].as_array().unwrap().len(), 1);
    let r = h
        .ok(&bot, "users.lookupByEmail", json!({"email": "ALICE@example.com"}))
        .await;
    assert_eq!(r["user"]["name"], "alice");
}
