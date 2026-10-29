//! `conversations.*` methods, plus the lookup and access checks other method
//! families share.

use serde_json::{Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::models::{Conversation, ConversationTypes, NewConversation, User};
use slack_emu_core::now;
use slack_emu_store::{ConversationQuery, TsRange};

use super::args::{self, bool_arg, opt_str, req_str};
use super::shapes::{self, ConvExtra};
use super::{ApiResult, Args, Caller, ConvAccess};
use crate::AppState;
use crate::events::{Audience, publish, publish_to_members};
use crate::services::{self, CreateError};

/// Error shorthand: `Err(SlackError::new(code).into())`.
pub(super) fn fail<T>(code: &str) -> ApiResult<T> {
    Err(SlackError::new(code).into())
}

fn looks_like_id(s: &str) -> bool {
    s.len() >= 9
        && matches!(s.as_bytes()[0], b'C' | b'D' | b'G')
        && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Resolves a channel argument (ID, `#name` or `name`) within the caller's workspace.
pub(super) async fn find_conversation(state: &AppState, caller: &Caller, raw: &str) -> ApiResult<Conversation> {
    let conv = if looks_like_id(raw) {
        state.store.get_conversation(raw).await?
    } else {
        let name = raw.trim_start_matches('#');
        state.store.conversation_by_name(&caller.team.id, name).await?
    };
    match conv {
        Some(c) if c.team_id == caller.team.id => Ok(c),
        _ => fail("channel_not_found"),
    }
}

/// A conversation the caller is allowed to know exists.
pub(super) struct Access {
    pub conv: Conversation,
    pub last_read: Option<String>,
}

impl Access {
    pub fn is_member(&self) -> bool {
        self.last_read.is_some()
    }

    pub fn require_member(&self) -> ApiResult<()> {
        if self.is_member() {
            Ok(())
        } else {
            fail("not_in_channel")
        }
    }

    /// Humans may read any public channel; bots must be members.
    pub fn require_read(&self, caller: &Caller) -> ApiResult<()> {
        if !self.is_member() && caller.is_bot() {
            fail("not_in_channel")
        } else {
            Ok(())
        }
    }

    pub fn require_not_archived(&self) -> ApiResult<()> {
        if self.conv.is_archived {
            fail("is_archived")
        } else {
            Ok(())
        }
    }

    pub fn require_channel(&self) -> ApiResult<()> {
        if self.conv.is_channel() {
            Ok(())
        } else {
            fail("method_not_supported_for_channel_type")
        }
    }
}

/// Loads `args[key]` as a conversation visible to the caller: any public
/// channel, or a private one/DM they belong to. Otherwise `channel_not_found`.
pub(super) async fn visible(
    state: &AppState,
    caller: &Caller,
    args: &Args,
    key: &str,
    access: Option<ConvAccess>,
) -> ApiResult<Access> {
    let raw = opt_str(args, key).ok_or_else(|| SlackError::new("channel_not_found"))?;
    let conv = find_conversation(state, caller, &raw).await?;
    if let Some(access) = access {
        caller.require_conv_scope(&conv, access)?;
    }
    let user = caller.user()?;
    let last_read = state.store.member_last_read(&conv.id, &user.id).await?;
    if (conv.is_private || !conv.is_channel()) && last_read.is_none() {
        return fail("channel_not_found");
    }
    Ok(Access { conv, last_read })
}

pub(super) async fn render(
    state: &AppState,
    caller: &Caller,
    conv: &Conversation,
    num_members: bool,
) -> ApiResult<Value> {
    let user = caller.user()?;
    let last_read = state.store.member_last_read(&conv.id, &user.id).await?;
    let num = if num_members && !conv.is_im {
        Some(state.store.member_count(&conv.id).await?)
    } else {
        None
    };
    Ok(shapes::conversation(
        conv,
        &ConvExtra {
            viewer: Some(user.id.clone()),
            is_member: Some(last_read.is_some()),
            num_members: num,
            last_read,
        },
    ))
}

/// Parses `types` (default `public_channel`).
pub(super) fn parse_types(args: &Args) -> ApiResult<ConversationTypes> {
    let list = args::list(args, "types");
    if list.is_empty() {
        return Ok(ConversationTypes {
            public: true,
            ..Default::default()
        });
    }
    let mut t = ConversationTypes::default();
    for ty in list {
        match ty.as_str() {
            "public_channel" => t.public = true,
            "private_channel" => t.private = true,
            "im" => t.im = true,
            "mpim" => t.mpim = true,
            _ => return fail("invalid_types"),
        }
    }
    Ok(t)
}

/// Lists conversations for `conversations.list` / `users.conversations`.
pub(super) async fn list_page(
    state: &AppState,
    caller: &Caller,
    viewer: &User,
    args: &Args,
    member_only: bool,
) -> ApiResult<Value> {
    let types = parse_types(args)?;
    for (wanted, scope) in [
        (types.public, "channels:read"),
        (types.private, "groups:read"),
        (types.im, "im:read"),
        (types.mpim, "mpim:read"),
    ] {
        if wanted {
            caller.require_scope(&[scope])?;
        }
    }
    let limit = args::limit(args, 100, 1000);
    let after = args::cursor(args, "id")?;
    let mut convs = state
        .store
        .list_conversations(ConversationQuery {
            team_id: &caller.team.id,
            viewer: &viewer.id,
            types,
            exclude_archived: bool_arg(args, "exclude_archived", false),
            member_only,
            after_id: after.as_deref(),
            limit: limit + 1,
        })
        .await?;
    let next = if convs.len() as i64 > limit {
        convs.truncate(limit as usize);
        convs.last().map(|c| args::encode_cursor("id", &c.id))
    } else {
        None
    };
    // Looking at someone else's memberships: hide private ones the caller isn't in.
    let me = caller.user()?;
    let mut out = Vec::with_capacity(convs.len());
    for c in &convs {
        let mine = state.store.member_last_read(&c.id, &me.id).await?;
        if viewer.id != me.id && (c.is_private || !c.is_channel()) && mine.is_none() {
            continue;
        }
        let num = if c.is_im {
            None
        } else {
            Some(state.store.member_count(&c.id).await?)
        };
        out.push(shapes::conversation(
            c,
            &ConvExtra {
                viewer: Some(me.id.clone()),
                is_member: Some(mine.is_some()),
                num_members: num,
                last_read: None,
            },
        ));
    }
    Ok(json!({"ok": true, "channels": out, "response_metadata": args::response_metadata(next)}))
}

/// Validates a new channel name the way Slack does.
fn validate_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim().trim_start_matches('#');
    if name.is_empty() {
        return fail("invalid_name_required");
    }
    if name.chars().count() > 80 {
        return fail("invalid_name_maxlength");
    }
    let ok_chars = name
        .chars()
        .all(|c| (c.is_alphanumeric() && !c.is_uppercase()) || c == '-' || c == '_');
    if !ok_chars {
        return fail("invalid_name_specials");
    }
    if name.starts_with("mpdm-") {
        return fail("invalid_name");
    }
    Ok(name.to_owned())
}

/// Announces a change to a conversation's metadata to clients (UI only).
async fn conversation_updated(state: &AppState, conv: &Conversation) -> ApiResult<()> {
    let obj = shapes::conversation(conv, &ConvExtra::default());
    let ev = json!({"type": "emu_conversation_updated", "channel": obj});
    if conv.is_private || !conv.is_channel() {
        publish_to_members(state, conv, &[], true, ev).await
    } else {
        publish(state, &conv.team_id, Audience::Team, true, ev);
        Ok(())
    }
}

/// Publishes a channel lifecycle event: `channel_<kind>` to the whole team for
/// public channels, `group_<kind>` to members for private ones.
async fn lifecycle_event(state: &AppState, conv: &Conversation, kind: &str, mut ev: Value) -> ApiResult<()> {
    if conv.is_private {
        ev["type"] = json!(format!("group_{kind}"));
        publish_to_members(state, conv, &[], false, ev).await
    } else {
        ev["type"] = json!(format!("channel_{kind}"));
        publish(state, &conv.team_id, Audience::Team, false, ev);
        Ok(())
    }
}

pub async fn list(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?.clone();
    list_page(state, caller, &me, args, false).await
}

pub async fn info(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::Read)).await?;
    let channel = render(state, caller, &a.conv, bool_arg(args, "include_num_members", false)).await?;
    Ok(json!({"ok": true, "channel": channel}))
}

pub async fn history(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::History)).await?;
    a.require_read(caller)?;
    let inclusive = bool_arg(args, "inclusive", false);
    let mut range = TsRange {
        oldest: args::opt_ts(args, "oldest")?,
        latest: args::opt_ts(args, "latest")?,
        oldest_inclusive: inclusive,
        latest_inclusive: inclusive,
    };
    if let Some(c) = args::cursor(args, "ts")? {
        range.latest = Some(c);
        range.latest_inclusive = false;
    }
    let limit = args::limit(args, 100, 999);
    let mut msgs = state.store.history(&a.conv.id, &range, limit + 1).await?;
    let has_more = msgs.len() as i64 > limit;
    msgs.truncate(limit as usize);
    let next = has_more.then(|| args::encode_cursor("ts", &msgs.last().expect("non-empty").ts));
    let rendered = shapes::messages(state, &a.conv.id, &msgs).await?;
    Ok(json!({
        "ok": true,
        "messages": rendered,
        "has_more": has_more,
        "pin_count": 0,
        "channel_actions_ts": null,
        "channel_actions_count": 0,
        "response_metadata": args::response_metadata(next),
    }))
}

pub async fn replies(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::History)).await?;
    a.require_read(caller)?;
    let ts = args::req_ts(args, "ts")?;
    let Some(msg) = state.store.get_message(&a.conv.id, &ts).await? else {
        return fail("thread_not_found");
    };
    let thread_ts = msg.thread_ts.clone().unwrap_or(msg.ts.clone());
    let parent = if thread_ts == msg.ts {
        msg
    } else {
        match state.store.get_message(&a.conv.id, &thread_ts).await? {
            Some(p) => p,
            None => return fail("thread_not_found"),
        }
    };

    let inclusive = bool_arg(args, "inclusive", false);
    let cursor = args::cursor(args, "ts")?;
    let mut range = TsRange {
        oldest: args::opt_ts(args, "oldest")?,
        latest: args::opt_ts(args, "latest")?,
        oldest_inclusive: inclusive,
        latest_inclusive: inclusive,
    };
    if let Some(c) = &cursor {
        range.oldest = Some(c.clone());
        range.oldest_inclusive = false;
    }
    let limit = args::limit(args, 1000, 1000);
    let mut msgs: Vec<_> = state
        .store
        .replies(&a.conv.id, &thread_ts, &range, limit + 2)
        .await?
        .into_iter()
        .filter(|m| m.ts != parent.ts)
        .collect();
    let has_more = msgs.len() as i64 > limit;
    msgs.truncate(limit as usize);
    let next = has_more.then(|| args::encode_cursor("ts", &msgs.last().expect("non-empty").ts));
    // The parent always leads the first page.
    if cursor.is_none() {
        msgs.insert(0, parent);
    }
    let rendered = shapes::messages(state, &a.conv.id, &msgs).await?;
    Ok(json!({
        "ok": true,
        "messages": rendered,
        "has_more": has_more,
        "response_metadata": args::response_metadata(next),
    }))
}

pub async fn members(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::Read)).await?;
    a.require_read(caller)?;
    let limit = args::limit(args, 100, 1000);
    let after = args::cursor(args, "id")?;
    let mut ids = state
        .store
        .list_members(&a.conv.id, after.as_deref(), limit + 1)
        .await?;
    let next = if ids.len() as i64 > limit {
        ids.truncate(limit as usize);
        ids.last().map(|id| args::encode_cursor("id", id))
    } else {
        None
    };
    Ok(json!({"ok": true, "members": ids, "response_metadata": args::response_metadata(next)}))
}

pub async fn create(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let user = caller.user()?;
    let is_private = bool_arg(args, "is_private", false);
    caller.require_scope(if is_private {
        &["groups:write"]
    } else {
        &["channels:manage"]
    })?;
    let name = validate_name(&opt_str(args, "name").unwrap_or_default())?;
    let conv = match services::create_channel_as(state, user, &name, is_private).await {
        Ok(c) => c,
        Err(CreateError::NameTaken) => return fail("name_taken"),
        Err(CreateError::Api(e)) => return Err(e),
    };
    let channel = render(state, caller, &conv, false).await?;
    Ok(json!({"ok": true, "channel": channel}))
}

pub async fn join(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", None).await?;
    if a.conv.is_private || !a.conv.is_channel() {
        return fail("method_not_supported_for_channel_type");
    }
    a.require_not_archived()?;
    let joined = services::join_conversation(state, &a.conv, &caller.user()?.id, None).await?;
    let channel = render(state, caller, &a.conv, false).await?;
    let mut out = json!({"ok": true, "channel": channel});
    if !joined {
        out["warning"] = json!("already_in_channel");
        out["response_metadata"] = json!({"warnings": ["already_in_channel"]});
    }
    Ok(out)
}

pub async fn leave(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    if a.conv.is_im {
        return fail("method_not_supported_for_channel_type");
    }
    if a.conv.is_general {
        return fail("cant_leave_general");
    }
    a.require_not_archived()?;
    if !services::leave_conversation(state, &a.conv, &caller.user()?.id).await? {
        return fail("not_in_channel");
    }
    Ok(json!({"ok": true}))
}

/// Looks up a user ID in the caller's workspace.
pub(super) async fn team_user(state: &AppState, caller: &Caller, id: &str) -> ApiResult<User> {
    match state.store.get_user(id).await? {
        Some(u) if u.team_id == caller.team.id && !u.deleted => Ok(u),
        _ => fail("user_not_found"),
    }
}

pub async fn invite(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::Invite)).await?;
    a.require_channel()?;
    a.require_member()?;
    a.require_not_archived()?;
    let me = caller.user()?;
    let ids = args::list(args, "users");
    if ids.is_empty() {
        return Err(super::ApiError::invalid_arguments("missing required field: users"));
    }
    let mut to_add = Vec::new();
    for id in &ids {
        let u = team_user(state, caller, id).await?;
        if u.id == me.id {
            return fail("cant_invite_self");
        }
        if state.store.member_last_read(&a.conv.id, &u.id).await?.is_none() {
            to_add.push(u.id);
        }
    }
    if to_add.is_empty() {
        return fail("already_in_channel");
    }
    for id in to_add {
        services::join_conversation(state, &a.conv, &id, Some(&me.id)).await?;
    }
    let channel = render(state, caller, &a.conv, false).await?;
    Ok(json!({"ok": true, "channel": channel}))
}

pub async fn kick(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    a.require_channel()?;
    a.require_member()?;
    let me = caller.user()?;
    let target = team_user(state, caller, &req_str(args, "user")?).await?;
    if target.id == me.id {
        return fail("cant_kick_self");
    }
    if a.conv.is_general {
        return fail("cant_kick_from_general");
    }
    if !services::leave_conversation(state, &a.conv, &target.id).await? {
        return fail("not_in_channel");
    }
    Ok(json!({"ok": true}))
}

pub async fn archive(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let mut a = visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    a.require_channel()?;
    if a.conv.is_general {
        return fail("cant_archive_general");
    }
    if a.conv.is_archived {
        return fail("already_archived");
    }
    a.require_member()?;
    let me = caller.user()?;
    services::post_system_message(state, &a.conv, &me.id, "channel_archive", "archived the channel").await?;
    a.conv.is_archived = true;
    state.store.update_conversation(&a.conv).await?;
    lifecycle_event(state, &a.conv, "archive", json!({"channel": a.conv.id, "user": me.id})).await?;
    conversation_updated(state, &a.conv).await?;
    Ok(json!({"ok": true}))
}

pub async fn unarchive(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let mut a = visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    a.require_channel()?;
    if !a.conv.is_archived {
        return fail("not_archived");
    }
    let me = caller.user()?;
    a.conv.is_archived = false;
    state.store.update_conversation(&a.conv).await?;
    services::post_system_message(state, &a.conv, &me.id, "channel_unarchive", "un-archived the channel").await?;
    lifecycle_event(
        state,
        &a.conv,
        "unarchive",
        json!({"channel": a.conv.id, "user": me.id}),
    )
    .await?;
    conversation_updated(state, &a.conv).await?;
    Ok(json!({"ok": true}))
}

pub async fn rename(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let mut a = visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    if a.conv.is_mpim || a.conv.is_im {
        return fail("method_not_supported_for_channel_type");
    }
    a.require_member()?;
    a.require_not_archived()?;
    let name = validate_name(&opt_str(args, "name").unwrap_or_default())?;
    let old = a.conv.name.clone().unwrap_or_default();
    if name == old {
        let channel = render(state, caller, &a.conv, false).await?;
        return Ok(json!({"ok": true, "channel": channel}));
    }
    a.conv.name = Some(name.clone());
    if let Err(e) = state.store.update_conversation(&a.conv).await {
        return match e {
            slack_emu_store::StoreError::Conflict(_) => fail("name_taken"),
            e => Err(e.into()),
        };
    }
    let me = caller.user()?;
    services::post_system_message(
        state,
        &a.conv,
        &me.id,
        "channel_name",
        &format!("renamed the channel from \"{old}\" to \"{name}\""),
    )
    .await?;
    lifecycle_event(
        state,
        &a.conv,
        "rename",
        json!({"channel": {"id": a.conv.id, "name": name, "created": a.conv.created}}),
    )
    .await?;
    conversation_updated(state, &a.conv).await?;
    let channel = render(state, caller, &a.conv, false).await?;
    Ok(json!({"ok": true, "channel": channel}))
}

/// Shared by setTopic and setPurpose.
async fn set_topic_like(state: &AppState, caller: &Caller, args: &Args, purpose: bool) -> ApiResult {
    let mut a = visible(state, caller, args, "channel", Some(ConvAccess::Topic)).await?;
    a.require_member()?;
    a.require_not_archived()?;
    let key = if purpose { "purpose" } else { "topic" };
    let value = opt_str(args, key).unwrap_or_default();
    if value.chars().count() > 250 {
        return fail("too_long");
    }
    let me = caller.user()?;
    let field = if purpose {
        &mut a.conv.purpose
    } else {
        &mut a.conv.topic
    };
    field.value = value.clone();
    field.creator = me.id.clone();
    field.last_set = now();
    state.store.update_conversation(&a.conv).await?;
    let (subtype, noun) = if purpose {
        ("channel_purpose", "channel description")
    } else {
        ("channel_topic", "channel topic")
    };
    let text = if value.is_empty() {
        format!("cleared the {noun}")
    } else {
        format!("set the {noun}: {value}")
    };
    services::post_system_message(state, &a.conv, &me.id, subtype, &text).await?;
    conversation_updated(state, &a.conv).await?;
    let channel = render(state, caller, &a.conv, false).await?;
    Ok(json!({"ok": true, "channel": channel}))
}

pub async fn set_topic(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    set_topic_like(state, caller, args, false).await
}

pub async fn set_purpose(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    set_topic_like(state, caller, args, true).await
}

/// Finds or creates the DM / group DM between the caller and `others`.
pub(super) async fn open_dm(state: &AppState, caller: &Caller, others: &[User]) -> ApiResult<(Conversation, bool)> {
    let me = caller.user()?;
    let mut participants: Vec<&User> = vec![me];
    for u in others {
        if !participants.iter().any(|p| p.id == u.id) {
            participants.push(u);
        }
    }
    if participants.len() > 9 {
        return fail("too_many_users");
    }
    let ids: Vec<String> = participants.iter().map(|u| u.id.clone()).collect();
    let key = Conversation::dm_key_for(&ids);
    if let Some(existing) = state.store.conversation_by_dm_key(&caller.team.id, &key).await? {
        return Ok((existing, false));
    }
    let is_mpim = participants.len() > 2;
    let name = is_mpim.then(|| {
        let mut sorted = participants.clone();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        let handles: Vec<&str> = sorted.iter().map(|u| u.name.as_str()).collect();
        format!("mpdm-{}-1", handles.join("--"))
    });
    let conv = state
        .store
        .create_conversation(
            NewConversation {
                team_id: caller.team.id.clone(),
                name,
                is_private: true,
                is_im: !is_mpim,
                is_mpim,
                is_general: false,
                creator: Some(me.id.clone()),
                dm_key: Some(key),
            },
            &ids,
        )
        .await?;
    // Each participant sees the DM from their own side (`user` is the peer).
    for p in &participants {
        let obj = shapes::conversation(
            &conv,
            &ConvExtra {
                viewer: Some(p.id.clone()),
                is_member: Some(true),
                ..Default::default()
            },
        );
        let ev = if is_mpim {
            json!({"type": "mpim_joined", "channel": obj})
        } else {
            json!({"type": "im_created", "user": p.id, "channel": obj})
        };
        publish(state, &conv.team_id, Audience::Users(vec![p.id.clone()]), is_mpim, ev);
    }
    Ok((conv, true))
}

pub async fn open(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    if opt_str(args, "channel").is_some() {
        let a = visible(state, caller, args, "channel", Some(ConvAccess::Read)).await?;
        a.require_member()?;
        let channel = render(state, caller, &a.conv, false).await?;
        return Ok(json!({"ok": true, "no_op": true, "already_open": true, "channel": channel}));
    }
    let ids = args::list(args, "users");
    if ids.is_empty() {
        return fail("users_list_not_supplied");
    }
    let mut others = Vec::new();
    for id in &ids {
        others.push(team_user(state, caller, id).await?);
    }
    let (conv, created) = open_dm(state, caller, &others).await?;
    let channel = render(state, caller, &conv, false).await?;
    let mut out = json!({"ok": true, "channel": channel});
    if !created {
        out["no_op"] = json!(true);
        out["already_open"] = json!(true);
    }
    Ok(out)
}

pub async fn close(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    // Open/closed DM state isn't modelled; closing is always a no-op.
    visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    Ok(json!({"ok": true, "no_op": true, "already_closed": true}))
}

pub async fn mark(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", Some(ConvAccess::Manage)).await?;
    a.require_member()?;
    let ts = args::req_ts(args, "ts")?;
    let me = caller.user()?;
    state.store.set_last_read(&a.conv.id, &me.id, &ts).await?;
    let kind = match a.conv.channel_type() {
        "im" => "im_marked",
        "mpim" | "group" => "group_marked",
        _ => "channel_marked",
    };
    publish(
        state,
        &a.conv.team_id,
        Audience::Users(vec![me.id.clone()]),
        true,
        json!({"type": kind, "channel": a.conv.id, "ts": ts}),
    );
    Ok(json!({"ok": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(validate_name("#dev-ops_2").unwrap(), "dev-ops_2");
        assert_eq!(validate_name("café").unwrap(), "café");
        assert_eq!(validate_name("").unwrap_err().code, "invalid_name_required");
        assert_eq!(validate_name("Dev").unwrap_err().code, "invalid_name_specials");
        assert_eq!(validate_name("a b").unwrap_err().code, "invalid_name_specials");
        assert_eq!(
            validate_name(&"a".repeat(81)).unwrap_err().code,
            "invalid_name_maxlength"
        );
    }

    #[test]
    fn ids() {
        assert!(looks_like_id("C0123ABCDEF"));
        assert!(!looks_like_id("general"));
        assert!(!looks_like_id("Cgeneral12"));
    }
}
