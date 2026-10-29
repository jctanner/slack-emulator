//! Files: the three-step upload, sharing, visibility, downloads and deletion.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::Harness;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

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

/// The path part of an emulator URL.
fn path(url: &str) -> String {
    url.split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, p)| format!("/{p}"))
        .unwrap()
}

/// POSTs bytes to an upload URL; `multipart` sends them as the Node SDK does.
async fn post_bytes(h: &Harness, url: &str, data: &[u8], multipart: bool) -> (StatusCode, String) {
    let req = if multipart {
        let boundary = "XyZboundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"body\"; filename=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        Request::post(path(url))
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap()
    } else {
        // slack_sdk sends raw bytes (urllib labels them form-encoded).
        Request::post(path(url))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(data.to_vec()))
            .unwrap()
    };
    let res = h.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// GETs a file URL with a token or session cookie; returns status, headers and bytes.
async fn fetch(h: &Harness, url: &str, who: Option<&str>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Request::get(path(url));
    if let Some(w) = who {
        req = if w.starts_with("xox") {
            req.header(header::AUTHORIZATION, format!("Bearer {w}"))
        } else {
            req.header(header::COOKIE, w)
        };
    }
    let res = h.app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, bytes)
}

/// The three-step upload as the SDKs do it. Returns the file ID.
async fn upload(h: &Harness, who: &str, name: &str, data: &[u8], complete: Value) -> String {
    let got = h
        .ok(
            who,
            "files.getUploadURLExternal",
            json!({"filename": name, "length": data.len()}),
        )
        .await;
    let (status, body) = post_bytes(h, got["upload_url"].as_str().unwrap(), data, false).await;
    assert_eq!((status, body), (StatusCode::OK, format!("OK - {}", data.len())));
    let id = got["file_id"].as_str().unwrap().to_owned();
    let mut args = complete;
    args["files"] = json!([{"id": id, "title": format!("Title of {name}")}]);
    let done = h.ok(who, "files.completeUploadExternal", args).await;
    assert_eq!(done["files"][0]["id"], id.as_str());
    id
}

#[tokio::test]
async fn bots_upload_share_and_people_download() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let general = general(&h, &alice).await;
    let (bot, bot_user) = h.bot("reporter").await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;

    let mut bus = h.state.bus.subscribe();
    let csv = b"name,count\nwidgets,3\n";
    let id = upload(
        &h,
        &bot,
        "report.csv",
        csv,
        json!({"channel_id": general, "initial_comment": "Nightly report"}),
    )
    .await;

    // The message carries the file, and file_shared fires.
    let history = h
        .ok(&bob, "conversations.history", json!({"channel": general, "limit": 1}))
        .await;
    let msg = &history["messages"][0];
    assert_eq!(msg["subtype"], "file_share");
    assert_eq!(msg["text"], "Nightly report");
    let file = &msg["files"][0];
    assert_eq!(file["id"], id.as_str());
    assert_eq!(file["name"], "report.csv");
    assert_eq!(file["title"], "Title of report.csv");
    assert_eq!(file["filetype"], "csv");
    assert_eq!(file["mimetype"], "text/csv");
    assert_eq!(file["size"], csv.len());
    assert_eq!(file["channels"][0], general.as_str());
    assert_eq!(file["shares"]["public"][&general][0]["ts"], msg["ts"]);
    let mut shared = None;
    while let Ok(ev) = bus.try_recv() {
        if ev.event["type"] == "file_shared" {
            shared = Some(ev.event.clone());
        }
    }
    let shared = shared.expect("file_shared");
    assert_eq!(shared["file_id"], id.as_str());
    assert_eq!(shared["channel_id"], general.as_str());

    // Downloads need a token or a session, like Slack's url_private.
    let url = file["url_private_download"].as_str().unwrap();
    assert_eq!(fetch(&h, url, None).await.0, StatusCode::UNAUTHORIZED);
    let (status, headers, bytes) = fetch(&h, url, Some(&bot)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, csv);
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename*=UTF-8''report.csv"
    );
    assert_eq!(headers[header::CONTENT_SECURITY_POLICY], "sandbox");
    assert_eq!(
        fetch(&h, file["url_private"].as_str().unwrap(), Some(&bob)).await.2,
        csv
    );
    // The permalink leads to it.
    let r = h.get(&path(file["permalink"].as_str().unwrap()), None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);

    let info = h.ok(&bob, "files.info", json!({"file": id})).await;
    assert_eq!(info["file"]["user"], bot_user.as_str());
    let listed = h.ok(&bob, "files.list", json!({"channel": general})).await;
    assert_eq!(listed["files"][0]["id"], id.as_str());
    assert_eq!(listed["paging"]["total"], 1);

    // The old upload method is gone, as on Slack.
    h.err(&bot, "files.upload", json!({"content": "x"}), "method_deprecated")
        .await;
}

#[tokio::test]
async fn multipart_uploads_images_and_private_files() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let bob_id = h.user_id(&bob).await;

    // A DM upload from the web UI (session), sent as multipart.
    let dm = h.ok(&alice, "conversations.open", json!({"users": bob_id})).await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let png = [0x89, b'P', b'N', b'G', 0, 1, 2, 3];
    let got = h
        .ok(
            &alice,
            "files.getUploadURLExternal",
            json!({"filename": "shot.png", "length": png.len()}),
        )
        .await;
    let url = got["upload_url"].as_str().unwrap().to_owned();
    assert_eq!(post_bytes(&h, &url, &png, true).await.0, StatusCode::OK);
    // Upload URLs work once.
    assert_eq!(post_bytes(&h, &url, &png, true).await.0, StatusCode::NOT_FOUND);
    let id = got["file_id"].as_str().unwrap();
    h.ok(
        &alice,
        "files.completeUploadExternal",
        json!({"files": [{"id": id}], "channel_id": dm}),
    )
    .await;
    let file = h.ok(&bob, "files.info", json!({"file": id})).await["file"].clone();
    assert_eq!(file["ims"][0], dm.as_str());
    assert_eq!(file["is_public"], false);
    assert!(file["thumb_360"].is_string(), "images get thumbnails");
    let (_, headers, bytes) = fetch(&h, file["url_private"].as_str().unwrap(), Some(&bob)).await;
    assert_eq!(bytes, png);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");

    // Outsiders can't see it, by API or URL.
    let carol = h.signup("carol@example.com").await;
    h.err(&carol, "files.info", json!({"file": id}), "file_not_found").await;
    assert_eq!(
        fetch(&h, file["url_private"].as_str().unwrap(), Some(&carol)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(h.ok(&carol, "files.list", json!({})).await["files"], json!([]));

    // Completing needs the bytes first; scopes are enforced.
    let pending = h
        .ok(
            &alice,
            "files.getUploadURLExternal",
            json!({"filename": "a.txt", "length": 1}),
        )
        .await;
    h.err(
        &alice,
        "files.completeUploadExternal",
        json!({"files": [{"id": pending["file_id"]}]}),
        "file_upload_not_completed",
    )
    .await;
}

#[tokio::test]
async fn deleted_files_leave_tombstones() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let general = general(&h, &alice).await;
    let id = upload(&h, &alice, "notes.txt", b"hello", json!({"channel_id": general})).await;

    h.err(&bob, "files.delete", json!({"file": id}), "cant_delete_file")
        .await;
    let mut bus = h.state.bus.subscribe();
    h.ok(&alice, "files.delete", json!({"file": id})).await;
    let ev = bus.recv().await.unwrap();
    assert_eq!(ev.event["type"], "file_deleted");
    assert_eq!(ev.event["file_id"], id.as_str());

    let history = h
        .ok(&bob, "conversations.history", json!({"channel": general, "limit": 1}))
        .await;
    assert_eq!(
        history["messages"][0]["files"][0],
        json!({"id": id, "mode": "tombstone"})
    );
    h.err(&alice, "files.info", json!({"file": id}), "file_not_found").await;
}

#[tokio::test]
async fn upload_size_is_limited() {
    let h = Harness::with_config(|c| c.max_file_bytes = 10).await;
    let alice = h.signup("alice@example.com").await;
    let r = h
        .call(
            &alice,
            "files.getUploadURLExternal",
            json!({"filename": "big.bin", "length": 11}),
        )
        .await;
    assert_eq!(r["error"], "invalid_arguments");
    let got = h
        .ok(
            &alice,
            "files.getUploadURLExternal",
            json!({"filename": "big.bin", "length": 5}),
        )
        .await;
    let (status, _) = post_bytes(&h, got["upload_url"].as_str().unwrap(), &[0u8; 50], false).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
