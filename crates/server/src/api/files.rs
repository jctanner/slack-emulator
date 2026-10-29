//! Files: Slack's upload flow (`files.getUploadURLExternal`, a POST of the
//! bytes to the returned URL, then `files.completeUploadExternal`), plus
//! `files.info`, `files.list` and `files.delete`, and the `url_private`
//! downloads, which need a token with `files:read` or a signed-in session.
//!
//! The old `files.upload` is answered with `method_deprecated`, as on Slack.

use std::collections::HashMap;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::CookieJar;
use serde_json::{Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::ids::{IdKind, new_id, new_secret};
use slack_emu_core::models::{Conversation, File, FileShare, Message};
use slack_emu_core::now;

use super::args::{self, opt_str, req_str};
use super::chat::{self, require_can_post};
use super::conversations::{fail, visible};
use super::{ApiError, ApiResult, Args, Caller};
use crate::AppState;
use crate::events::{Audience, publish, publish_to_members};
use crate::services;

/// Image types shown inline; everything else downloads as an attachment, so
/// an uploaded page can't run scripts on the emulator's origin.
const INLINE_IMAGES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Slack's `filetype` and `pretty_type` for a file name (or a snippet type).
fn filetype(name: &str, snippet_type: Option<&str>) -> (String, String) {
    let ext = snippet_type.map(str::to_owned).unwrap_or_else(|| {
        name.rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    });
    let (ft, pretty) = match ext.as_str() {
        "txt" | "text" | "log" => ("text", "Plain Text"),
        "csv" => ("csv", "CSV"),
        "json" => ("json", "JSON"),
        "md" | "markdown" => ("markdown", "Markdown (raw)"),
        "yaml" | "yml" => ("yaml", "YAML"),
        "xml" => ("xml", "XML"),
        "html" | "htm" => ("html", "HTML"),
        "py" | "python" => ("python", "Python"),
        "js" | "javascript" => ("javascript", "JavaScript"),
        "ts" | "typescript" => ("typescript", "TypeScript"),
        "rs" | "rust" => ("rust", "Rust"),
        "go" => ("go", "Go"),
        "java" => ("java", "Java"),
        "sh" | "shell" | "bash" => ("shell", "Shell"),
        "sql" => ("sql", "SQL"),
        "diff" | "patch" => ("diff", "Diff"),
        "pdf" => ("pdf", "PDF"),
        "png" => ("png", "PNG"),
        "jpg" | "jpeg" => ("jpg", "JPEG"),
        "gif" => ("gif", "GIF"),
        "webp" => ("webp", "WebP"),
        "svg" => ("svg", "SVG"),
        "zip" => ("zip", "Zip"),
        "gz" | "tgz" => ("gzip", "GZip"),
        _ => ("binary", "Binary"),
    };
    (ft.into(), pretty.into())
}

fn pretty_type(f: &File) -> String {
    filetype(&f.name, f.snippet_type.as_deref()).1
}

/// Percent-encodes a file name for URL paths.
fn encode(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub fn url_private(state: &AppState, f: &File) -> String {
    format!(
        "{}/files-pri/{}-{}/{}",
        state.config.public_url,
        f.team_id,
        f.id,
        encode(&f.name)
    )
}

/// The file object as `files.info` and messages show it.
pub async fn file_object(state: &AppState, f: &File, shares: &[FileShare]) -> ApiResult<Value> {
    let base = &state.config.public_url;
    let private = url_private(state, f);
    let mut v = json!({
        "id": f.id,
        "created": f.created,
        "timestamp": f.created,
        "name": f.name,
        "title": f.title,
        "mimetype": f.mimetype,
        "filetype": f.filetype,
        "pretty_type": pretty_type(f),
        "user": f.user_id,
        "user_team": f.team_id,
        "editable": f.snippet_type.is_some(),
        "size": f.size,
        "mode": if f.snippet_type.is_some() { "snippet" } else { "hosted" },
        "is_external": false,
        "external_type": "",
        "is_public": false,
        "public_url_shared": false,
        "display_as_bot": false,
        "username": "",
        "url_private": private,
        "url_private_download": format!("{base}/files-pri/{}-{}/download/{}", f.team_id, f.id, encode(&f.name)),
        "permalink": format!("{base}/files/{}/{}/{}", f.user_id, f.id, encode(&f.name)),
        "comments_count": 0,
        "has_rich_preview": false,
        "file_access": "visible",
    });
    if let Some(alt) = &f.alt_txt {
        v["alt_txt"] = json!(alt);
    }
    if INLINE_IMAGES.contains(&f.mimetype.as_str()) {
        for size in [64, 80, 160, 360, 480, 720, 960, 1024] {
            v[format!("thumb_{size}")] = json!(private);
        }
    }
    let (mut channels, mut groups, mut ims) = (Vec::new(), Vec::new(), Vec::new());
    let mut public_shares = serde_json::Map::new();
    let mut private_shares = serde_json::Map::new();
    for s in shares {
        let Some(conv) = state.store.get_conversation(&s.channel_id).await? else {
            continue;
        };
        let entry = json!({
            "ts": s.ts,
            "channel_name": conv.name.clone().unwrap_or_default(),
            "team_id": conv.team_id,
            "share_user_id": f.user_id,
            "source": "UNKNOWN",
        });
        let public = conv.is_channel() && !conv.is_private;
        let (list, bucket) = if public {
            (&mut channels, &mut public_shares)
        } else if conv.is_im {
            (&mut ims, &mut private_shares)
        } else {
            (&mut groups, &mut private_shares)
        };
        if !list.contains(&conv.id) {
            list.push(conv.id.clone());
        }
        bucket
            .entry(conv.id.clone())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("array")
            .push(entry);
    }
    v["is_public"] = json!(!channels.is_empty());
    v["channels"] = json!(channels);
    v["groups"] = json!(groups);
    v["ims"] = json!(ims);
    let mut share_obj = serde_json::Map::new();
    if !public_shares.is_empty() {
        share_obj.insert("public".into(), Value::Object(public_shares));
    }
    if !private_shares.is_empty() {
        share_obj.insert("private".into(), Value::Object(private_shares));
    }
    v["shares"] = Value::Object(share_obj);
    Ok(v)
}

/// File objects for messages, keyed by file ID. Deleted files are absent.
pub async fn files_for_messages(state: &AppState, msgs: &[Message]) -> ApiResult<HashMap<String, Value>> {
    let ids: Vec<String> = msgs.iter().flat_map(|m| m.file_ids.iter().cloned()).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let files = state.store.get_files(&ids).await?;
    let shares = state.store.file_shares(&ids).await?;
    let mut out = HashMap::new();
    for f in files {
        let mine: Vec<FileShare> = shares.iter().filter(|s| s.file_id == f.id).cloned().collect();
        out.insert(f.id.clone(), file_object(state, &f, &mine).await?);
    }
    Ok(out)
}

/// Whether the caller may see a conversation's files: members can; people can
/// also see public channels (bots must be members, as for history).
async fn can_see_conversation(state: &AppState, caller: &Caller, conv: &Conversation) -> ApiResult<bool> {
    let me = caller.user()?;
    if conv.team_id != caller.team.id {
        return Ok(false);
    }
    if state.store.member_last_read(&conv.id, &me.id).await?.is_some() {
        return Ok(true);
    }
    Ok(conv.is_channel() && !conv.is_private && !caller.is_bot())
}

/// A file the caller may see (they uploaded it, or it's in a conversation they can see).
async fn visible_file(state: &AppState, caller: &Caller, id: &str) -> ApiResult<(File, Vec<FileShare>)> {
    let me = caller.user()?;
    let Some(f) = state.store.get_file(id).await?.filter(|f| f.team_id == caller.team.id) else {
        return fail("file_not_found");
    };
    let shares = state.store.file_shares(std::slice::from_ref(&f.id)).await?;
    if f.user_id == me.id && (f.completed || f.uploaded) {
        return Ok((f, shares));
    }
    if f.completed {
        for s in &shares {
            if let Some(conv) = state.store.get_conversation(&s.channel_id).await?
                && can_see_conversation(state, caller, &conv).await?
            {
                return Ok((f, shares));
            }
        }
    }
    fail("file_not_found")
}

pub async fn get_upload_url(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let me = caller.user()?;
    let filename = req_str(a, "filename")?;
    let length: i64 = req_str(a, "length")?
        .parse()
        .map_err(|_| ApiError::invalid_arguments("length must be a number of bytes"))?;
    if length < 0 || length as usize > state.config.max_file_bytes {
        return Err(ApiError::invalid_arguments(format!(
            "length must be at most {} bytes (EMU_MAX_FILE_BYTES)",
            state.config.max_file_bytes
        )));
    }
    let snippet_type = opt_str(a, "snippet_type");
    let (ft, _) = filetype(&filename, snippet_type.as_deref());
    let mimetype = if snippet_type.is_some() {
        "text/plain".to_owned()
    } else {
        mime_guess::from_path(&filename).first_or_octet_stream().to_string()
    };
    let token = new_secret();
    let file = File {
        id: new_id(IdKind::File),
        team_id: caller.team.id.clone(),
        user_id: me.id.clone(),
        bot_id: caller.token.as_ref().and_then(|t| t.bot_id.clone()),
        title: filename.clone(),
        name: filename,
        mimetype,
        filetype: ft,
        snippet_type,
        // The Node SDK sends `alt_text`, the docs say `alt_txt`.
        alt_txt: opt_str(a, "alt_txt").or_else(|| opt_str(a, "alt_text")),
        size: length,
        created: now(),
        upload_token: Some(token.clone()),
        uploaded: false,
        completed: false,
    };
    state.store.create_file(&file).await?;
    Ok(json!({
        "ok": true,
        "upload_url": format!("{}/upload/v1/{token}", state.config.public_url),
        "file_id": file.id,
    }))
}

pub async fn complete_upload(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let me = caller.user()?;
    let listed = args::json(a, "files")?.ok_or_else(|| ApiError::invalid_arguments("missing required field: files"))?;
    let Some(listed) = listed.as_array().filter(|l| !l.is_empty()) else {
        return Err(ApiError::invalid_arguments(
            "files must be a non-empty array of {id, title}",
        ));
    };
    let mut files = Vec::new();
    for item in listed {
        let id = item["id"].as_str().unwrap_or_default();
        let Some(mut f) = state
            .store
            .get_file(id)
            .await?
            .filter(|f| f.team_id == caller.team.id && f.user_id == me.id && !f.completed)
        else {
            return fail("file_not_found");
        };
        if !f.uploaded {
            return fail("file_upload_not_completed");
        }
        if let Some(title) = item["title"].as_str().filter(|t| !t.is_empty()) {
            f.title = title.to_owned();
        }
        files.push(f);
    }

    // Where to share: `channel_id`, or a comma-separated `channels`.
    let mut targets = args::list(a, "channels");
    if let Some(c) = opt_str(a, "channel_id") {
        targets.insert(0, c);
    }
    let mut convs = Vec::new();
    for channel in &targets {
        let mut ca = Args::new();
        ca.insert("channel".into(), json!(channel));
        let access = visible(state, caller, &ca, "channel", None).await?;
        require_can_post(caller, &access)?;
        convs.push(access.conv);
    }
    let blocks = chat::content(a)?.blocks;
    let thread_ts = args::opt_ts(a, "thread_ts")?;

    for f in &mut files {
        f.completed = true;
        state.store.update_file(f).await?;
    }
    let file_ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    for conv in &convs {
        let text = opt_str(a, "initial_comment").unwrap_or_default();
        let mut msg = chat::new_message(state, caller, conv, text)?;
        msg.subtype = Some("file_share".into());
        msg.file_ids = file_ids.clone();
        msg.blocks = blocks.clone();
        if let Some(tts) = &thread_ts {
            let Some(parent) = state.store.get_message(&conv.id, tts).await? else {
                return fail("thread_not_found");
            };
            msg.thread_ts = Some(parent.thread_ts.unwrap_or(parent.ts));
        }
        state.store.insert_message(&msg).await?;
        for id in &file_ids {
            let share = FileShare {
                file_id: id.clone(),
                channel_id: conv.id.clone(),
                ts: msg.ts.clone(),
            };
            state.store.add_file_share(&share).await?;
        }
        services::announce_message(state, conv, &msg).await?;
        for id in &file_ids {
            let ev = json!({
                "type": "file_shared",
                "file_id": id,
                "user_id": me.id,
                "file": {"id": id},
                "channel_id": conv.id,
                "event_ts": state.clock.next(),
            });
            publish_to_members(state, conv, &[], false, ev).await?;
        }
    }
    let out: Vec<Value> = files.iter().map(|f| json!({"id": f.id, "title": f.title})).collect();
    Ok(json!({"ok": true, "files": out}))
}

pub async fn info(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let id = req_str(a, "file")?;
    let (f, shares) = visible_file(state, caller, &id).await?;
    Ok(json!({
        "ok": true,
        "file": file_object(state, &f, &shares).await?,
        "comments": [],
        "response_metadata": {"next_cursor": ""},
    }))
}

/// `types` filter: `all`, `images`, `pdfs`, `snippets`, `zips`, `spaces`, `gdocs`.
fn type_matches(f: &File, types: &[String]) -> bool {
    types.is_empty()
        || types.iter().any(|t| match t.as_str() {
            "all" => true,
            "images" => f.mimetype.starts_with("image/"),
            "pdfs" => f.filetype == "pdf",
            "snippets" => f.snippet_type.is_some(),
            "zips" => matches!(f.filetype.as_str(), "zip" | "gzip"),
            _ => false,
        })
}

pub async fn list(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let channel = opt_str(a, "channel");
    let user = opt_str(a, "user");
    let types = args::list(a, "types");
    let ts_from = opt_str(a, "ts_from").and_then(|s| s.parse::<f64>().ok());
    let ts_to = opt_str(a, "ts_to").and_then(|s| s.parse::<f64>().ok());
    let count = opt_str(a, "count")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(100)
        .min(1000);
    let page = opt_str(a, "page")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1);

    let all = state.store.list_files(&caller.team.id).await?;
    let ids: Vec<String> = all.iter().map(|f| f.id.clone()).collect();
    let shares = state.store.file_shares(&ids).await?;
    let me = caller.user()?;
    let mut seen_convs: HashMap<String, bool> = HashMap::new();
    let mut matching = Vec::new();
    for f in all {
        let mine: Vec<FileShare> = shares.iter().filter(|s| s.file_id == f.id).cloned().collect();
        if user.as_ref().is_some_and(|u| *u != f.user_id)
            || !type_matches(&f, &types)
            || ts_from.is_some_and(|t| (f.created as f64) < t)
            || ts_to.is_some_and(|t| (f.created as f64) > t)
            || channel
                .as_ref()
                .is_some_and(|c| !mine.iter().any(|s| s.channel_id == *c))
        {
            continue;
        }
        let mut visible = f.user_id == me.id;
        for s in &mine {
            if visible {
                break;
            }
            let ok = match seen_convs.get(&s.channel_id) {
                Some(ok) => *ok,
                None => {
                    let ok = match state.store.get_conversation(&s.channel_id).await? {
                        Some(conv) => can_see_conversation(state, caller, &conv).await?,
                        None => false,
                    };
                    seen_convs.insert(s.channel_id.clone(), ok);
                    ok
                }
            };
            visible = ok;
        }
        if visible {
            matching.push((f, mine));
        }
    }
    let total = matching.len();
    let mut files = Vec::new();
    for (f, mine) in matching.iter().skip((page - 1) * count).take(count) {
        files.push(file_object(state, f, mine).await?);
    }
    Ok(json!({
        "ok": true,
        "files": files,
        "paging": {"count": count, "total": total, "page": page, "pages": total.div_ceil(count).max(1)},
    }))
}

pub async fn delete(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let id = req_str(a, "file")?;
    let (f, shares) = visible_file(state, caller, &id).await?;
    let me = caller.user()?;
    let admin = !caller.is_bot() && (me.is_admin || me.is_owner);
    if f.user_id != me.id && !admin {
        return fail("cant_delete_file");
    }
    state.store.delete_file(&f.id).await?;
    // Everyone who could see it: members of the conversations it was in, and the uploader.
    let mut audience = vec![f.user_id.clone()];
    for s in &shares {
        for m in state.store.list_members(&s.channel_id, None, i64::MAX).await? {
            if !audience.contains(&m) {
                audience.push(m);
            }
        }
    }
    let ev = json!({"type": "file_deleted", "file_id": f.id, "event_ts": state.clock.next()});
    publish(state, &f.team_id, Audience::Users(audience), false, ev);
    Ok(json!({"ok": true}))
}

pub async fn upload_v1(_state: &AppState, _caller: &Caller, _a: &Args) -> ApiResult {
    Err(ApiError::from(SlackError::new("method_deprecated")).with(
        "response_metadata",
        json!({"messages": ["[ERROR] files.upload was retired; use files.getUploadURLExternal and files.completeUploadExternal (files_upload_v2 / files.uploadV2 in the SDKs)"]}),
    ))
}

pub fn routes(state: &AppState) -> Router<AppState> {
    // Room for multipart framing around the largest allowed file.
    let limit = state.config.max_file_bytes + 1024 * 1024;
    Router::new()
        .route("/upload/v1/{token}", post(upload).layer(DefaultBodyLimit::max(limit)))
        .route("/files-pri/{key}/{name}", get(download))
        .route("/files-pri/{key}/download/{name}", get(download_attachment))
        .route("/files/{user}/{file}/{name}", get(permalink))
}

fn text(status: StatusCode, body: impl Into<String>) -> Response {
    (status, body.into()).into_response()
}

/// The upload URL: takes the file's bytes, raw (`slack_sdk`) or as a multipart form (the Node SDK).
async fn upload(State(state): State<AppState>, Path(token): Path<String>, req: Request) -> Response {
    let Ok(Some(mut f)) = state.store.file_by_upload_token(&token).await else {
        return text(StatusCode::NOT_FOUND, "invalid upload URL");
    };
    let is_multipart = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("multipart/form-data"));
    let data = if is_multipart {
        let Ok(mut form) = Multipart::from_request(req, &state).await else {
            return text(StatusCode::BAD_REQUEST, "invalid multipart body");
        };
        // The first part carrying data (the SDKs send exactly one).
        let mut data = None;
        while let Ok(Some(field)) = form.next_field().await {
            match field.bytes().await {
                Ok(b) if !b.is_empty() => {
                    data = Some(b);
                    break;
                }
                Ok(_) => continue,
                Err(_) => return text(StatusCode::PAYLOAD_TOO_LARGE, "file too large (EMU_MAX_FILE_BYTES)"),
            }
        }
        data.unwrap_or_default()
    } else {
        match Bytes::from_request(req, &state).await {
            Ok(b) => b,
            Err(_) => return text(StatusCode::PAYLOAD_TOO_LARGE, "file too large (EMU_MAX_FILE_BYTES)"),
        }
    };
    if data.len() > state.config.max_file_bytes {
        return text(StatusCode::PAYLOAD_TOO_LARGE, "file too large (EMU_MAX_FILE_BYTES)");
    }
    if state.store.set_file_data(&f.id, &data).await.is_err() {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "couldn't store the file");
    }
    f.size = data.len() as i64;
    f.uploaded = true;
    f.upload_token = None;
    if state.store.update_file(&f).await.is_err() {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "couldn't store the file");
    }
    text(StatusCode::OK, format!("OK - {}", data.len()))
}

async fn download(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Response {
    serve(state, path.0.0, headers, jar, false).await
}

async fn download_attachment(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Response {
    serve(state, path.0.0, headers, jar, true).await
}

/// `url_private`: like Slack, needs `Authorization: Bearer <token>` (with
/// `files:read`) or the web UI's session.
async fn serve(
    State(state): State<AppState>,
    key: String,
    headers: HeaderMap,
    jar: CookieJar,
    attachment: bool,
) -> Response {
    let result: ApiResult<Response> = async {
        let Some((team, id)) = key.split_once('-') else {
            return fail("file_not_found");
        };
        let mut a = Args::new();
        a.insert("team_id".into(), json!(team));
        let caller = super::resolve_caller(&state, &headers, &jar, &a)
            .await?
            .ok_or_else(SlackError::not_authed)?;
        if caller.team.id != team {
            return fail("file_not_found");
        }
        caller.require_scope(&["files:read"])?;
        let (f, _) = visible_file(&state, &caller, id).await?;
        let Some(data) = state.store.file_data(&f.id).await? else {
            return fail("file_not_found");
        };
        let inline = !attachment && INLINE_IMAGES.contains(&f.mimetype.as_str());
        let mut res = data.into_response();
        let h = res.headers_mut();
        let content_type = if inline {
            f.mimetype.as_str()
        } else {
            "application/octet-stream"
        };
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).expect("ascii"),
        );
        let disposition = format!(
            "{}; filename*=UTF-8''{}",
            if inline { "inline" } else { "attachment" },
            encode(&f.name)
        );
        h.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition).expect("encoded"),
        );
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
        Ok(res)
    }
    .await;
    result.unwrap_or_else(|e| {
        let status = if e.code == "not_authed" || e.code == "invalid_auth" {
            StatusCode::UNAUTHORIZED
        } else if e.code == "missing_scope" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::NOT_FOUND
        };
        let mut e = e;
        e.status = status;
        e.into_response()
    })
}

/// A file's `permalink` opens its `url_private`.
async fn permalink(
    State(state): State<AppState>,
    Path((_user, id, _name)): Path<(String, String, String)>,
) -> Response {
    match state.store.get_file(&id).await {
        Ok(Some(f)) => Redirect::to(&url_private(&state, &f)).into_response(),
        _ => text(StatusCode::NOT_FOUND, "file not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_names() {
        assert_eq!(filetype("Report.CSV", None).0, "csv");
        assert_eq!(filetype("x", Some("python")).0, "python");
        assert_eq!(filetype("archive.tar", None).0, "binary");
        assert_eq!(encode("a b/ü.txt"), "a%20b%2F%C3%BC.txt");
    }
}
