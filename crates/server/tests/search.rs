//! Search.

mod common;

use common::Harness;
use serde_json::{Value, json};

fn texts(r: &Value) -> Vec<String> {
    r["messages"]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn searching_messages_and_files() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let bob_id = h.user_id(&bob).await;
    let ops = h.ok(&alice, "conversations.create", json!({"name": "ops"})).await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.ok(&bob, "conversations.join", json!({"channel": ops})).await;
    let secret = h
        .ok(
            &alice,
            "conversations.create",
            json!({"name": "secret", "is_private": true}),
        )
        .await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dm = h.ok(&alice, "conversations.open", json!({"users": bob_id})).await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let post = |who: String, channel: String, text: &'static str| {
        let h = &h;
        async move {
            h.ok(&who, "chat.postMessage", json!({"channel": channel, "text": text}))
                .await
        }
    };
    let first = post(alice.clone(), ops.clone(), "Deploy started, see https://ci.example/42").await;
    post(bob.clone(), ops.clone(), "deploy FAILED on db migration").await;
    post(alice.clone(), secret.clone(), "secret deploy plan").await;
    post(bob.clone(), dm.clone(), "can you rerun the deploy?").await;
    h.ok(
        &bob,
        "chat.postMessage",
        json!({"channel": ops, "text": "deploy retry is green", "thread_ts": first["ts"]}),
    )
    .await;
    h.ok(&alice, "pins.add", json!({"channel": ops, "timestamp": first["ts"]}))
        .await;

    let search = |who: &String, query: &str| {
        let h = &h;
        let args = json!({"query": query});
        let who = who.clone();
        async move { h.ok(&who, "search.messages", args).await }
    };
    let r = search(&alice, "deploy").await;
    assert_eq!(r["messages"]["total"], 5);
    let m = &r["messages"]["matches"][0];
    assert_eq!(m["text"], "deploy retry is green");
    assert_eq!(m["channel"]["name"], "ops");
    assert!(m["permalink"].as_str().unwrap().contains("/archives/"));

    // Bob isn't in #secret.
    assert_eq!(search(&bob, "deploy").await["messages"]["total"], 4);
    assert_eq!(
        texts(&search(&alice, "deploy in:#ops from:@bob").await),
        ["deploy retry is green", "deploy FAILED on db migration"]
    );
    assert_eq!(
        texts(&search(&alice, "deploy in:@bob").await),
        ["can you rerun the deploy?"]
    );
    assert_eq!(
        texts(&search(&alice, r#""db migration""#).await),
        ["deploy FAILED on db migration"]
    );
    assert_eq!(
        texts(&search(&alice, "deploy has:link").await),
        ["Deploy started, see https://ci.example/42"]
    );
    assert_eq!(
        texts(&search(&alice, "deploy has:pin").await),
        ["Deploy started, see https://ci.example/42"]
    );
    assert_eq!(search(&alice, "deploy is:thread").await["messages"]["total"], 2);
    assert_eq!(search(&alice, "deploy from:@nobody").await["messages"]["total"], 0);
    assert_eq!(search(&alice, "deploy before:2000-01-01").await["messages"]["total"], 0);
    assert_eq!(search(&alice, "deploy on:today").await["messages"]["total"], 5);

    let paged = h
        .ok(
            &alice,
            "search.messages",
            json!({"query": "deploy", "count": 2, "page": 3, "sort_dir": "asc"}),
        )
        .await;
    assert_eq!(
        paged["messages"]["paging"],
        json!({"count": 2, "total": 5, "page": 3, "pages": 3})
    );
    assert_eq!(texts(&paged), ["deploy retry is green"]);

    // Files match by name or title.
    let got = h
        .ok(
            &alice,
            "files.getUploadURLExternal",
            json!({"filename": "deploy-log.txt", "length": 2}),
        )
        .await;
    let path = got["upload_url"]
        .as_str()
        .unwrap()
        .split_once("8080")
        .unwrap()
        .1
        .to_owned();
    let req = axum::http::Request::post(path)
        .body(axum::body::Body::from("ok"))
        .unwrap();
    h.send(req).await;
    h.ok(
        &alice,
        "files.completeUploadExternal",
        json!({"files": [{"id": got["file_id"]}], "channel_id": ops}),
    )
    .await;
    let files = h.ok(&bob, "search.files", json!({"query": "LOG"})).await;
    assert_eq!(files["files"]["matches"][0]["name"], "deploy-log.txt");
    let all = h.ok(&bob, "search.all", json!({"query": "deploy in:#ops"})).await;
    assert_eq!(all["messages"]["total"], 3);
    assert_eq!(all["files"]["total"], 1);

    // Bots can't search, as on Slack.
    let (bot, _) = h.bot("searcher").await;
    h.err(
        &bot,
        "search.messages",
        json!({"query": "deploy"}),
        "not_allowed_token_type",
    )
    .await;
}
