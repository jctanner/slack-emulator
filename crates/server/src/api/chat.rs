//! `chat.*` methods.

use serde_json::{Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::blocks::{Surface, validate_blocks};
use slack_emu_core::models::{App, Conversation, Message};

use super::args::{self, bool_arg, opt_str};
use super::conversations::{Access, fail, open_dm, team_user, visible};
use super::{ApiError, ApiResult, Args, Caller, shapes};
use crate::AppState;
use crate::events::publish_to_members;
use crate::{interactions, services};

const MAX_TEXT: usize = 40_000;

/// Resolves `channel` for posting. A user ID means "DM this user".
async fn post_target(state: &AppState, caller: &Caller, args: &Args) -> ApiResult<Access> {
    let raw = opt_str(args, "channel").ok_or_else(|| slack_emu_core::SlackError::new("channel_not_found"))?;
    if raw.starts_with('U') || raw.starts_with('W') {
        let other = team_user(state, caller, &raw).await?;
        let (conv, _) = open_dm(state, caller, &[other]).await?;
        let last_read = state.store.member_last_read(&conv.id, &caller.user()?.id).await?;
        return Ok(Access { conv, last_read });
    }
    visible(state, caller, args, "channel", None).await
}

/// Rewrites `@handle` and `#channel` into Slack's `<@U…>` / `<#C…|name>` syntax
/// (`link_names`). With `labels`, user mentions keep the handle too
/// (`<@U…|name>`), as in slash command text with `should_escape`.
pub(crate) async fn link_names(state: &AppState, team_id: &str, text: &str, labels: bool) -> ApiResult<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    let mut prev: Option<char> = None;
    while let Some((i, c)) = chars.next() {
        let boundary = prev.is_none_or(|p| p.is_whitespace() || p == '(');
        if (c == '@' || c == '#') && boundary {
            let rest = &text[i + 1..];
            let end = rest
                .find(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-')))
                .unwrap_or(rest.len());
            let word = rest[..end].trim_end_matches('.');
            if !word.is_empty() {
                let replacement = if c == '@' {
                    match word {
                        "here" | "channel" | "everyone" => Some(format!("<!{word}>")),
                        _ => state
                            .store
                            .list_users(team_id)
                            .await?
                            .into_iter()
                            .find(|u| u.name == word)
                            .map(|u| {
                                if labels {
                                    format!("<@{}|{}>", u.id, u.name)
                                } else {
                                    format!("<@{}>", u.id)
                                }
                            }),
                    }
                } else {
                    state
                        .store
                        .conversation_by_name(team_id, word)
                        .await?
                        .filter(|c| !c.is_private)
                        .map(|c| format!("<#{}|{}>", c.id, word))
                };
                if let Some(r) = replacement {
                    out.push_str(&r);
                    for _ in 0..word.chars().count() {
                        chars.next();
                    }
                    prev = word.chars().last();
                    continue;
                }
            }
        }
        out.push(c);
        prev = Some(c);
    }
    Ok(out)
}

/// Text, blocks and attachments for a new or updated message.
pub(crate) struct Content {
    pub text: Option<String>,
    pub blocks: Option<Value>,
    pub attachments: Option<Value>,
}

impl Content {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.blocks.is_none() && self.attachments.is_none()
    }
}

fn invalid_blocks(errors: Vec<String>) -> ApiError {
    ApiError::from(SlackError::new("invalid_blocks"))
        .with("errors", json!(errors))
        .with("response_metadata", json!({"messages": errors}))
}

/// Reads and validates `text`, `blocks` and `attachments`, filling in missing
/// block and action IDs.
pub(crate) fn content(args: &Args) -> ApiResult<Content> {
    let text = opt_str(args, "text");
    let mut blocks = args::json(args, "blocks")?;
    let mut attachments = args::json(args, "attachments")?;
    if let Some(b) = &mut blocks {
        if !b.is_array() {
            return fail("invalid_blocks");
        }
        let errors = validate_blocks(b, Surface::Message, "/blocks");
        if !errors.is_empty() {
            return Err(invalid_blocks(errors));
        }
    }
    if let Some(a) = &mut attachments {
        let Some(list) = a.as_array_mut() else {
            return fail("invalid_attachments");
        };
        let mut errors = Vec::new();
        for (i, att) in list.iter_mut().enumerate() {
            if let Some(b) = att.get_mut("blocks") {
                errors.extend(validate_blocks(
                    b,
                    Surface::Message,
                    &format!("/attachments/{i}/blocks"),
                ));
            }
        }
        if !errors.is_empty() {
            return Err(invalid_blocks(errors));
        }
    }
    if text.as_ref().is_some_and(|t| t.chars().count() > MAX_TEXT) {
        return fail("msg_too_long");
    }
    Ok(Content {
        text,
        blocks,
        attachments,
    })
}

/// Creates a message from the caller in `conv`.
fn new_message(state: &AppState, caller: &Caller, conv: &Conversation, text: String) -> ApiResult<Message> {
    let user = caller.user()?;
    let token = caller.token.as_ref();
    Ok(build_message(
        state,
        conv,
        Some(user.id.clone()),
        token.and_then(|t| t.bot_id.clone()),
        token.and_then(|t| t.app_id.clone()),
        text,
    ))
}

/// A message from an app's bot user (for responses outside a Web API call).
pub(crate) fn app_message(state: &AppState, conv: &Conversation, app: &App, c: Content) -> Message {
    let mut msg = build_message(
        state,
        conv,
        app.bot_user_id.clone(),
        app.bot_id.clone(),
        Some(app.id.clone()),
        c.text.unwrap_or_default(),
    );
    msg.blocks = c.blocks;
    msg.attachments = c.attachments;
    msg
}

pub(crate) fn build_message(
    state: &AppState,
    conv: &Conversation,
    user_id: Option<String>,
    bot_id: Option<String>,
    app_id: Option<String>,
    text: String,
) -> Message {
    Message {
        channel_id: conv.id.clone(),
        ts: state.clock.next(),
        team_id: conv.team_id.clone(),
        user_id,
        bot_id,
        app_id,
        subtype: None,
        text,
        blocks: None,
        attachments: None,
        username: None,
        icons: None,
        metadata: None,
        client_msg_id: None,
        thread_ts: None,
        reply_broadcast: false,
        edited_ts: None,
        edited_by: None,
    }
}

/// Whether the caller may post to this conversation: members can, and bots
/// with `chat:write.public` can post to any public channel.
fn require_can_post(caller: &Caller, a: &Access) -> ApiResult<()> {
    let public_ok = !caller.is_session()
        && a.conv.is_channel()
        && !a.conv.is_private
        && caller.token.as_ref().is_some_and(|t| t.has_scope("chat:write.public"));
    if !public_ok {
        a.require_member()?;
    }
    a.require_not_archived()
}

/// Display overrides need `chat:write.customize`; without it they're ignored.
fn customize(caller: &Caller, args: &Args, msg: &mut Message) {
    if caller.is_bot()
        && caller
            .token
            .as_ref()
            .is_some_and(|t| t.has_scope("chat:write.customize"))
    {
        msg.username = opt_str(args, "username");
        msg.icons = match (opt_str(args, "icon_emoji"), opt_str(args, "icon_url")) {
            (Some(e), _) => Some(json!({"emoji": e})),
            (None, Some(u)) => Some(json!({"image_48": u, "image_72": u})),
            _ => None,
        };
    }
}

pub async fn post_message(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = post_target(state, caller, args).await?;
    require_can_post(caller, &a)?;
    let c = content(args)?;
    if c.is_empty() {
        return fail("no_text");
    }
    let mut text = c.text.unwrap_or_default();
    if bool_arg(args, "link_names", false) {
        text = link_names(state, &caller.team.id, &text, false).await?;
    }

    let mut msg = new_message(state, caller, &a.conv, text)?;
    msg.blocks = c.blocks;
    msg.attachments = c.attachments;
    msg.metadata = args::json(args, "metadata")?;
    msg.client_msg_id = opt_str(args, "client_msg_id");
    customize(caller, args, &mut msg);
    if let Some(tts) = args::opt_ts(args, "thread_ts")? {
        let Some(parent) = state.store.get_message(&a.conv.id, &tts).await? else {
            return fail("thread_not_found");
        };
        // Replying to a reply lands in the same thread.
        msg.thread_ts = Some(parent.thread_ts.unwrap_or(parent.ts));
        if bool_arg(args, "reply_broadcast", false) {
            msg.reply_broadcast = true;
            msg.subtype = Some("thread_broadcast".into());
        }
    }

    state.store.insert_message(&msg).await?;
    services::announce_message(state, &a.conv, &msg).await?;
    let rendered = shapes::one_message(state, &msg).await?;
    Ok(json!({"ok": true, "channel": a.conv.id, "ts": msg.ts, "message": rendered}))
}

/// `chat.postEphemeral`: a message only `user` sees, until they reload. Not
/// stored and not delivered to apps.
pub async fn post_ephemeral(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", None).await?;
    require_can_post(caller, &a)?;
    let target = opt_str(args, "user").ok_or_else(|| SlackError::new("user_not_found"))?;
    let target = team_user(state, caller, &target).await?;
    if state.store.member_last_read(&a.conv.id, &target.id).await?.is_none() {
        return fail("user_not_in_channel");
    }
    let c = content(args)?;
    if c.is_empty() {
        return fail("no_text");
    }
    let mut text = c.text.unwrap_or_default();
    if bool_arg(args, "link_names", false) {
        text = link_names(state, &caller.team.id, &text, false).await?;
    }
    let mut msg = new_message(state, caller, &a.conv, text)?;
    msg.blocks = c.blocks;
    msg.attachments = c.attachments;
    msg.thread_ts = args::opt_ts(args, "thread_ts")?;
    customize(caller, args, &mut msg);
    interactions::send_ephemeral(state, &a.conv, &target.id, &msg).await?;
    Ok(json!({"ok": true, "message_ts": msg.ts}))
}

pub async fn me_message(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", None).await?;
    a.require_member()?;
    a.require_not_archived()?;
    let text = content(args)?
        .text
        .ok_or_else(|| slack_emu_core::SlackError::new("no_text"))?;
    let mut msg = new_message(state, caller, &a.conv, text)?;
    msg.subtype = Some("me_message".into());
    state.store.insert_message(&msg).await?;
    services::announce_message(state, &a.conv, &msg).await?;
    Ok(json!({"ok": true, "channel": a.conv.id, "ts": msg.ts}))
}

/// Subtypes people can edit or delete (system messages like `channel_join` are fixed).
fn is_user_content(m: &Message) -> bool {
    matches!(
        m.subtype.as_deref(),
        None | Some("thread_broadcast" | "me_message" | "bot_message" | "file_share")
    )
}

async fn load_message(state: &AppState, caller: &Caller, args: &Args) -> ApiResult<(Access, Message)> {
    let a = visible(state, caller, args, "channel", None).await?;
    let ts = args::req_ts(args, "ts")?;
    match state.store.get_message(&a.conv.id, &ts).await? {
        Some(m) => Ok((a, m)),
        None => fail("message_not_found"),
    }
}

async fn message_changed(state: &AppState, conv: &Conversation, before: Value, after: &Message) -> ApiResult<()> {
    let event_ts = state.clock.next();
    let new = shapes::one_message(state, after).await?;
    publish_to_members(
        state,
        conv,
        &[],
        false,
        json!({
            "type": "message",
            "subtype": "message_changed",
            "hidden": true,
            "channel": conv.id,
            "channel_type": conv.channel_type(),
            "ts": event_ts,
            "event_ts": event_ts,
            "message": new,
            "previous_message": before,
        }),
    )
    .await
}

pub async fn update(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let (a, mut msg) = load_message(state, caller, args).await?;
    let me = caller.user()?;
    if msg.user_id.as_deref() != Some(&me.id) || !is_user_content(&msg) {
        return fail("cant_update_message");
    }
    a.require_not_archived()?;
    let mut c = content(args)?;
    if c.is_empty() {
        return fail("no_text");
    }
    if let Some(t) = &mut c.text
        && bool_arg(args, "link_names", false)
    {
        *t = link_names(state, &caller.team.id, t, false).await?;
    }
    if let Some(md) = args::json(args, "metadata")? {
        msg.metadata = Some(md);
    }
    // Slack only marks people's edits; apps update their messages silently.
    let editor = (!caller.is_bot()).then_some(me.id.as_str());
    apply_update(state, &a.conv, &mut msg, c, editor).await?;
    let rendered = shapes::one_message(state, &msg).await?;
    Ok(json!({"ok": true, "channel": a.conv.id, "ts": msg.ts, "text": msg.text, "message": rendered}))
}

/// Replaces a message's content and announces the change.
pub(crate) async fn apply_update(
    state: &AppState,
    conv: &Conversation,
    msg: &mut Message,
    c: Content,
    editor: Option<&str>,
) -> ApiResult<()> {
    let before = shapes::one_message(state, msg).await?;
    if let Some(t) = c.text {
        msg.text = t;
    }
    if c.blocks.is_some() {
        msg.blocks = c.blocks;
    }
    if c.attachments.is_some() {
        msg.attachments = c.attachments;
    }
    if let Some(editor) = editor {
        msg.edited_ts = Some(state.clock.next());
        msg.edited_by = Some(editor.to_owned());
    }
    state.store.update_message(msg).await?;
    message_changed(state, conv, before, msg).await
}

async fn message_deleted(state: &AppState, conv: &Conversation, before: Value, ts: &str) -> ApiResult<()> {
    let event_ts = state.clock.next();
    publish_to_members(
        state,
        conv,
        &[],
        false,
        json!({
            "type": "message",
            "subtype": "message_deleted",
            "hidden": true,
            "channel": conv.id,
            "channel_type": conv.channel_type(),
            "ts": event_ts,
            "event_ts": event_ts,
            "deleted_ts": ts,
            "previous_message": before,
        }),
    )
    .await
}

pub async fn delete(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let (a, msg) = load_message(state, caller, args).await?;
    let me = caller.user()?;
    let own = msg.user_id.as_deref() == Some(&me.id);
    let admin = !caller.is_bot() && (me.is_admin || me.is_owner);
    if !(own || admin) || !is_user_content(&msg) {
        return fail("cant_delete_message");
    }
    remove_message(state, &a.conv, msg.clone()).await?;
    Ok(json!({"ok": true, "channel": a.conv.id, "ts": msg.ts}))
}

/// Deletes a message, leaving a tombstone if it's a thread parent with replies.
pub(crate) async fn remove_message(state: &AppState, conv: &Conversation, mut msg: Message) -> ApiResult<()> {
    let before = shapes::one_message(state, &msg).await?;

    let has_replies = msg.is_thread_parent()
        && state
            .store
            .thread_summaries(&conv.id, std::slice::from_ref(&msg.ts))
            .await?
            .get(&msg.ts)
            .is_some_and(|s| s.reply_count > 0);
    if has_replies {
        // Keep the thread reachable: replace the parent with a tombstone.
        msg.subtype = Some("tombstone".into());
        msg.text = "This message was deleted.".into();
        msg.user_id = Some("USLACKBOT".into());
        msg.blocks = None;
        msg.attachments = None;
        msg.edited_ts = None;
        msg.edited_by = None;
        state.store.update_message(&msg).await?;
        message_changed(state, conv, before, &msg).await?;
    } else {
        state.store.delete_message(&conv.id, &msg.ts).await?;
        message_deleted(state, conv, before, &msg.ts).await?;
        if let Some(parent_ts) = msg.thread_ts.as_deref().filter(|_| msg.is_reply()) {
            // A tombstoned parent goes away with its last reply.
            if let Some(parent) = state.store.get_message(&conv.id, parent_ts).await?
                && parent.subtype.as_deref() == Some("tombstone")
                && state
                    .store
                    .thread_summaries(&conv.id, std::slice::from_ref(&parent.ts))
                    .await?
                    .is_empty()
            {
                let before = shapes::one_message(state, &parent).await?;
                state.store.delete_message(&conv.id, &parent.ts).await?;
                message_deleted(state, conv, before, &parent.ts).await?;
            } else {
                services::thread_updated(state, conv, parent_ts).await?;
            }
        }
    }
    Ok(())
}

pub fn permalink(state: &AppState, conv: &Conversation, msg: &Message) -> String {
    let mut url = format!(
        "{}/archives/{}/p{}",
        state.config.public_url,
        conv.id,
        msg.ts.replace('.', "")
    );
    if msg.is_reply() {
        url.push_str(&format!(
            "?thread_ts={}&cid={}",
            msg.thread_ts.as_deref().unwrap_or_default(),
            conv.id
        ));
    }
    url
}

pub async fn get_permalink(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", None).await?;
    let ts = args::req_ts(args, "message_ts")?;
    let Some(msg) = state.store.get_message(&a.conv.id, &ts).await? else {
        return fail("message_not_found");
    };
    Ok(json!({"ok": true, "channel": a.conv.id, "permalink": permalink(state, &a.conv, &msg)}))
}

/// Resolves a `/archives/{channel}/p{ts}` permalink to the conversation's
/// workspace and the message ts.
pub async fn resolve_permalink(state: &AppState, channel: &str, p: &str) -> ApiResult<Option<(String, String)>> {
    let digits = p.strip_prefix('p').unwrap_or(p);
    if digits.len() != 16 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let ts = format!("{}.{}", &digits[..10], &digits[10..]);
    Ok(state.store.get_conversation(channel).await?.map(|c| (c.team_id, ts)))
}
