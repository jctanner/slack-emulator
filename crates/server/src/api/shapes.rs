//! JSON shapes of Slack objects, as returned by the Web API.

use std::collections::HashMap;

use serde_json::{Value, json};
use slack_emu_core::models::{App, Conversation, Message, Reaction, ThreadSummary, User, Workspace};

use super::ApiResult;
use crate::AppState;

pub fn team(ws: &Workspace) -> Value {
    json!({
        "id": ws.id,
        "name": ws.name,
        "domain": ws.domain,
    })
}

pub fn profile(u: &User) -> Value {
    json!({
        "real_name": u.real_name,
        "real_name_normalized": u.real_name,
        "display_name": u.display_name,
        "display_name_normalized": u.display_name,
        "email": u.email,
        "team": u.team_id,
    })
}

/// A user object as returned by `users.info` / `users.list`.
pub fn user(u: &User) -> Value {
    user_with_app(u, None)
}

/// A user object; bot users also get their app's `bot_id` and `api_app_id`.
pub fn user_with_app(u: &User, app: Option<&App>) -> Value {
    let mut v = json!({
        "id": u.id,
        "team_id": u.team_id,
        "name": u.name,
        "deleted": u.deleted,
        "real_name": u.real_name,
        "tz": u.tz,
        "is_admin": u.is_admin,
        "is_owner": u.is_owner,
        "is_bot": u.is_bot,
        "is_app_user": false,
        "updated": u.created,
        "profile": profile(u),
    });
    if let Some(app) = app {
        v["profile"]["bot_id"] = json!(app.bot_id);
        v["profile"]["api_app_id"] = json!(app.id);
        v["profile"]["always_active"] = json!(app.manifest().features.bot_user.is_some_and(|b| b.always_online));
    }
    v
}

/// Viewer-specific fields to add to a conversation object.
#[derive(Debug, Default)]
pub struct ConvExtra {
    pub viewer: Option<String>,
    pub is_member: Option<bool>,
    pub num_members: Option<i64>,
    pub last_read: Option<String>,
}

/// The other participant of a DM (or the viewer, for a self-DM).
pub fn im_peer(c: &Conversation, viewer: Option<&str>) -> Option<String> {
    let key = c.dm_key.as_deref()?;
    let ids: Vec<&str> = key.split(',').collect();
    ids.iter()
        .find(|id| Some(**id) != viewer)
        .or(ids.first())
        .map(|s| (*s).to_owned())
}

/// A conversation object as returned by `conversations.info` / `conversations.list`.
pub fn conversation(c: &Conversation, extra: &ConvExtra) -> Value {
    let mut v = if c.is_im {
        json!({
            "id": c.id,
            "created": c.created,
            "is_im": true,
            "is_org_shared": false,
            "context_team_id": c.team_id,
            "user": im_peer(c, extra.viewer.as_deref()),
            "is_user_deleted": false,
            "priority": 0,
        })
    } else {
        json!({
            "id": c.id,
            "name": c.name,
            "name_normalized": c.name,
            "is_channel": c.is_channel(),
            "is_group": c.is_mpim,
            "is_im": false,
            "is_mpim": c.is_mpim,
            "is_private": c.is_private,
            "created": c.created,
            "is_archived": c.is_archived,
            "is_general": c.is_general,
            "unlinked": 0,
            "is_shared": false,
            "is_org_shared": false,
            "is_ext_shared": false,
            "is_pending_ext_shared": false,
            "pending_shared": [],
            "context_team_id": c.team_id,
            "creator": c.creator.clone().unwrap_or_default(),
            "previous_names": [],
            "topic": {"value": c.topic.value, "creator": c.topic.creator, "last_set": c.topic.last_set},
            "purpose": {"value": c.purpose.value, "creator": c.purpose.creator, "last_set": c.purpose.last_set},
        })
    };
    if let Some(m) = extra.is_member {
        v["is_member"] = json!(m);
    }
    if let Some(n) = extra.num_members {
        v["num_members"] = json!(n);
    }
    if let Some(lr) = &extra.last_read {
        v["last_read"] = json!(lr);
    }
    v
}

/// A message object as it appears in `conversations.history` and events.
pub fn message(m: &Message, thread: Option<&ThreadSummary>, reactions: Option<&[Reaction]>) -> Value {
    let mut v = json!({
        "type": "message",
        "ts": m.ts,
        "text": m.text,
        "team": m.team_id,
    });
    let set = |v: &mut Value, k: &str, val: Option<Value>| {
        if let Some(val) = val {
            v[k] = val;
        }
    };
    set(&mut v, "user", m.user_id.as_ref().map(|u| json!(u)));
    set(&mut v, "subtype", m.subtype.as_ref().map(|s| json!(s)));
    set(&mut v, "bot_id", m.bot_id.as_ref().map(|s| json!(s)));
    set(&mut v, "app_id", m.app_id.as_ref().map(|s| json!(s)));
    set(&mut v, "username", m.username.as_ref().map(|s| json!(s)));
    set(&mut v, "icons", m.icons.clone());
    set(&mut v, "blocks", m.blocks.clone());
    set(&mut v, "attachments", m.attachments.clone());
    set(&mut v, "metadata", m.metadata.clone());
    set(&mut v, "client_msg_id", m.client_msg_id.as_ref().map(|s| json!(s)));
    set(&mut v, "thread_ts", m.thread_ts.as_ref().map(|s| json!(s)));
    if let (Some(user), Some(ts)) = (&m.edited_by, &m.edited_ts) {
        v["edited"] = json!({"user": user, "ts": ts});
    }
    if m.subtype.as_deref() == Some("tombstone") {
        v["hidden"] = json!(true);
    }
    if m.is_thread_parent() {
        let t = thread.cloned().unwrap_or_default();
        v["reply_count"] = json!(t.reply_count);
        v["reply_users_count"] = json!(t.reply_users.len());
        v["reply_users"] = json!(t.reply_users);
        if let Some(latest) = t.latest_reply {
            v["latest_reply"] = json!(latest);
        }
        v["is_locked"] = json!(false);
        v["subscribed"] = json!(false);
    }
    if let Some(rs) = reactions.filter(|r| !r.is_empty()) {
        v["reactions"] = rs
            .iter()
            .map(|r| json!({"name": r.name, "users": r.users, "count": r.users.len()}))
            .collect();
    }
    v
}

/// Renders messages from one channel, attaching thread summaries, reactions,
/// and `parent_user_id` on replies.
pub async fn messages(state: &AppState, channel_id: &str, msgs: &[Message]) -> ApiResult<Vec<Value>> {
    let parents: Vec<String> = msgs
        .iter()
        .filter(|m| m.is_thread_parent())
        .map(|m| m.ts.clone())
        .collect();
    let all_ts: Vec<String> = msgs.iter().map(|m| m.ts.clone()).collect();
    let threads = state.store.thread_summaries(channel_id, &parents).await?;
    let reactions = state.store.reactions(channel_id, &all_ts).await?;

    // Authors of thread parents referenced by replies.
    let mut parent_users: HashMap<String, Option<String>> =
        msgs.iter().map(|m| (m.ts.clone(), m.user_id.clone())).collect();
    for m in msgs.iter().filter(|m| m.is_reply()) {
        let pts = m.thread_ts.clone().expect("reply has thread_ts");
        if let std::collections::hash_map::Entry::Vacant(e) = parent_users.entry(pts) {
            let user = state
                .store
                .get_message(channel_id, e.key())
                .await?
                .and_then(|p| p.user_id);
            e.insert(user);
        }
    }

    // Files, by ID; a message naming a deleted one shows a tombstone.
    let files = super::files::files_for_messages(state, msgs).await?;

    // Bot messages carry their bot's profile.
    let mut bots: HashMap<String, Option<Value>> = HashMap::new();
    for bot_id in msgs.iter().filter_map(|m| m.bot_id.as_ref()) {
        if !bots.contains_key(bot_id) {
            let profile = state
                .store
                .app_by_bot_id(bot_id)
                .await?
                .map(|app| super::apps::bot_profile(&app, true));
            bots.insert(bot_id.clone(), profile);
        }
    }

    Ok(msgs
        .iter()
        .map(|m| {
            let mut v = message(m, threads.get(&m.ts), reactions.get(&m.ts).map(Vec::as_slice));
            if m.is_reply()
                && let Some(Some(pu)) = m.thread_ts.as_ref().and_then(|t| parent_users.get(t))
            {
                v["parent_user_id"] = json!(pu);
            }
            if let Some(Some(profile)) = m.bot_id.as_ref().and_then(|b| bots.get(b)) {
                v["bot_profile"] = profile.clone();
            }
            if !m.file_ids.is_empty() {
                v["files"] = m
                    .file_ids
                    .iter()
                    .map(|id| {
                        files
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| json!({"id": id, "mode": "tombstone"}))
                    })
                    .collect();
                v["upload"] = json!(false);
                v["display_as_bot"] = json!(false);
            }
            v
        })
        .collect())
}

/// Renders a single message with its thread summary and reactions.
pub async fn one_message(state: &AppState, m: &Message) -> ApiResult<Value> {
    Ok(messages(state, &m.channel_id, std::slice::from_ref(m))
        .await?
        .pop()
        .expect("one in, one out"))
}
