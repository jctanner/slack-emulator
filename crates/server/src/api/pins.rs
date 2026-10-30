//! `pins.*` (pinned messages) and `bookmarks.*` (links under a channel's header).

use serde_json::{Value, json};
use slack_emu_core::ids::{IdKind, new_id};
use slack_emu_core::models::{Bookmark, Conversation, Message, Pin};
use slack_emu_core::now;

use super::args::{self, opt_str, req_str};
use super::conversations::{Access, fail, visible};
use super::{ApiResult, Args, Caller, shapes};
use crate::AppState;
use crate::events::publish_to_members;
use crate::services;

/// Slack allows 100 pins per conversation.
const MAX_PINS: usize = 100;

fn pinnable(m: &Message) -> bool {
    matches!(
        m.subtype.as_deref(),
        None | Some("thread_broadcast" | "me_message" | "bot_message" | "file_share")
    )
}

/// The conversation and message a pin call names (`channel` + `timestamp`).
async fn pin_target(state: &AppState, caller: &Caller, a: &Args) -> ApiResult<(Access, Message)> {
    let access = visible(state, caller, a, "channel", None).await?;
    access.require_member()?;
    let ts = args::req_ts(a, "timestamp")?;
    match state.store.get_message(&access.conv.id, &ts).await? {
        Some(m) => Ok((access, m)),
        None => fail("message_not_found"),
    }
}

/// The `item` of pin events and `pins.list`.
async fn pin_item(state: &AppState, conv: &Conversation, msg: &Message, pin: &Pin) -> ApiResult<Value> {
    Ok(json!({
        "type": "message",
        "channel": conv.id,
        "created": pin.created,
        "created_by": pin.pinned_by,
        "message": shapes::one_message(state, msg).await?,
    }))
}

pub async fn add(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let (access, msg) = pin_target(state, caller, a).await?;
    access.require_not_archived()?;
    if !pinnable(&msg) {
        return fail("not_pinnable");
    }
    let conv = &access.conv;
    let count = state.store.pins(&conv.id).await?.len();
    if count >= MAX_PINS {
        return fail("too_many_pins");
    }
    let me = caller.user()?;
    let pin = Pin {
        channel_id: conv.id.clone(),
        ts: msg.ts.clone(),
        pinned_by: me.id.clone(),
        created: now(),
    };
    if !state.store.add_pin(&pin).await? {
        return fail("already_pinned");
    }
    let item = pin_item(state, conv, &msg, &pin).await?;
    let ev = json!({
        "type": "pin_added",
        "user": me.id,
        "channel_id": conv.id,
        "item": item,
        "item_user": msg.user_id,
        "pin_count": count + 1,
        "pinned_info": {"channel": conv.id, "pinned_by": me.id, "pinned_ts": pin.created},
        "event_ts": state.clock.next(),
    });
    publish_to_members(state, conv, &[], false, ev).await?;
    // As in Slack, the conversation notes the pin.
    let text = format!("<@{}> pinned a message to this channel.", me.id);
    services::post_system_message(state, conv, &me.id, "pinned_item", &text).await?;
    Ok(json!({"ok": true}))
}

pub async fn remove(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let (access, msg) = pin_target(state, caller, a).await?;
    let conv = &access.conv;
    let pinned = state.store.pins(&conv.id).await?;
    let Some(pin) = pinned.iter().find(|p| p.ts == msg.ts).cloned() else {
        return fail("no_pin");
    };
    state.store.remove_pin(&conv.id, &msg.ts).await?;
    let me = caller.user()?;
    let ev = json!({
        "type": "pin_removed",
        "user": me.id,
        "channel_id": conv.id,
        "item": pin_item(state, conv, &msg, &pin).await?,
        "item_user": msg.user_id,
        "pin_count": pinned.len() - 1,
        "has_pins": pinned.len() > 1,
        "event_ts": state.clock.next(),
    });
    publish_to_members(state, conv, &[], false, ev).await?;
    Ok(json!({"ok": true}))
}

pub async fn list(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let access = visible(state, caller, a, "channel", None).await?;
    access.require_read(caller)?;
    let conv = &access.conv;
    let mut items = Vec::new();
    // Newest pins first.
    for pin in state.store.pins(&conv.id).await?.iter().rev() {
        if let Some(msg) = state.store.get_message(&conv.id, &pin.ts).await? {
            items.push(pin_item(state, conv, &msg, pin).await?);
        }
    }
    Ok(json!({"ok": true, "items": items}))
}

fn bookmark_json(b: &Bookmark, team_id: &str, rank: usize) -> Value {
    json!({
        "id": b.id,
        "channel_id": b.channel_id,
        "title": b.title,
        "link": b.link,
        "emoji": b.emoji,
        "icon_url": null,
        "type": "link",
        "entity_id": null,
        "date_created": b.created,
        "date_updated": b.updated,
        // Slack ranks with opaque sortable strings.
        "rank": format!("{:04}", rank),
        "last_updated_by_user_id": b.updated_by,
        "last_updated_by_team_id": team_id,
        "shortcut_id": null,
        "app_id": b.app_id,
    })
}

/// Bookmark methods name the conversation `channel_id`. Changing bookmarks needs membership.
async fn bookmark_channel(state: &AppState, caller: &Caller, a: &Args, write: bool) -> ApiResult<Access> {
    let access = visible(state, caller, a, "channel_id", None).await?;
    if write {
        access.require_member()?;
        access.require_not_archived()?;
    } else {
        access.require_read(caller)?;
    }
    Ok(access)
}

fn valid_link(link: &str) -> bool {
    (link.starts_with("https://") || link.starts_with("http://"))
        && link.len() > 8
        && !link.contains(char::is_whitespace)
}

/// Clients redraw the bookmarks bar (Slack has no public event for this).
async fn bookmarks_changed(state: &AppState, conv: &Conversation) -> ApiResult<()> {
    publish_to_members(
        state,
        conv,
        &[],
        true,
        json!({"type": "emu_bookmarks_changed", "channel": conv.id}),
    )
    .await
}

pub async fn bookmark_add(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let access = bookmark_channel(state, caller, a, true).await?;
    let title = req_str(a, "title")?;
    if req_str(a, "type")? != "link" {
        return fail("invalid_bookmark_type");
    }
    let link = req_str(a, "link")?;
    if !valid_link(&link) {
        return fail("invalid_link");
    }
    let me = caller.user()?;
    let b = Bookmark {
        // Slack's bookmark IDs look like `Bk01ABCDEF9`.
        id: format!("Bk{}", &new_id(IdKind::Channel)[1..]),
        channel_id: access.conv.id.clone(),
        title,
        link,
        emoji: opt_str(a, "emoji"),
        app_id: caller.token.as_ref().and_then(|t| t.app_id.clone()),
        created: now(),
        updated: now(),
        updated_by: me.id.clone(),
    };
    state.store.add_bookmark(&b).await?;
    bookmarks_changed(state, &access.conv).await?;
    let rank = state.store.bookmarks(&access.conv.id).await?.len();
    Ok(json!({"ok": true, "bookmark": bookmark_json(&b, &caller.team.id, rank)}))
}

async fn existing_bookmark(state: &AppState, access: &Access, a: &Args) -> ApiResult<Bookmark> {
    let id = req_str(a, "bookmark_id")?;
    match state.store.get_bookmark(&id).await? {
        Some(b) if b.channel_id == access.conv.id => Ok(b),
        _ => fail("not_found"),
    }
}

pub async fn bookmark_edit(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let access = bookmark_channel(state, caller, a, true).await?;
    let mut b = existing_bookmark(state, &access, a).await?;
    if let Some(title) = opt_str(a, "title") {
        b.title = title;
    }
    if let Some(link) = opt_str(a, "link") {
        if !valid_link(&link) {
            return fail("invalid_link");
        }
        b.link = link;
    }
    if let Some(emoji) = a.get("emoji") {
        b.emoji = emoji.as_str().filter(|e| !e.is_empty()).map(str::to_owned);
    }
    b.updated = now();
    b.updated_by = caller.user()?.id.clone();
    state.store.update_bookmark(&b).await?;
    bookmarks_changed(state, &access.conv).await?;
    let all = state.store.bookmarks(&access.conv.id).await?;
    let rank = all.iter().position(|x| x.id == b.id).unwrap_or(0) + 1;
    Ok(json!({"ok": true, "bookmark": bookmark_json(&b, &caller.team.id, rank)}))
}

pub async fn bookmark_remove(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let access = bookmark_channel(state, caller, a, true).await?;
    let b = existing_bookmark(state, &access, a).await?;
    state.store.remove_bookmark(&b.id).await?;
    bookmarks_changed(state, &access.conv).await?;
    Ok(json!({"ok": true}))
}

pub async fn bookmark_list(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let access = bookmark_channel(state, caller, a, false).await?;
    let bookmarks: Vec<Value> = state
        .store
        .bookmarks(&access.conv.id)
        .await?
        .iter()
        .enumerate()
        .map(|(i, b)| bookmark_json(b, &caller.team.id, i + 1))
        .collect();
    Ok(json!({"ok": true, "bookmarks": bookmarks}))
}
