//! Web API method implementations.

use serde_json::{Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::ids::TokenKind;

use super::{
    ApiResult, Args, Caller, apps, chat, conversations, files, inspector, interactive, pins, reactions, scheduled,
    usergroups, users, views,
};
use crate::AppState;

/// Which credentials may call a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callers {
    /// No authentication needed.
    Anyone,
    /// Bot/user tokens and the web UI.
    Standard,
    /// Only `xapp-` app-level tokens.
    AppToken,
    /// Only the web UI (emulator administration and Slack's internal client methods).
    Session,
}

pub struct MethodSpec {
    pub name: &'static str,
    pub callers: Callers,
    /// Token scopes, any one of which allows the call (empty: none needed, or
    /// checked per conversation inside the method).
    pub scopes: &'static [&'static str],
}

const fn m(name: &'static str, scopes: &'static [&'static str]) -> MethodSpec {
    MethodSpec {
        name,
        callers: Callers::Standard,
        scopes,
    }
}

const fn session(name: &'static str) -> MethodSpec {
    MethodSpec {
        name,
        callers: Callers::Session,
        scopes: &[],
    }
}

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "api.test",
        callers: Callers::Anyone,
        scopes: &[],
    },
    MethodSpec {
        name: "apps.connections.open",
        callers: Callers::AppToken,
        scopes: &["connections:write"],
    },
    session("apps.manifest.create"),
    session("apps.manifest.delete"),
    session("apps.manifest.export"),
    session("apps.manifest.update"),
    session("apps.manifest.validate"),
    m("auth.revoke", &[]),
    m("auth.test", &[]),
    m("bookmarks.add", &["bookmarks:write"]),
    m("bookmarks.edit", &["bookmarks:write"]),
    m("bookmarks.list", &["bookmarks:read"]),
    m("bookmarks.remove", &["bookmarks:write"]),
    m("bots.info", &["users:read"]),
    session("chat.command"),
    m("chat.delete", &["chat:write"]),
    m("chat.deleteScheduledMessage", &["chat:write"]),
    m("chat.getPermalink", &[]),
    m("chat.meMessage", &["chat:write"]),
    m("chat.postEphemeral", &["chat:write"]),
    m("chat.postMessage", &["chat:write"]),
    m("chat.scheduleMessage", &["chat:write"]),
    m("chat.scheduledMessages.list", &["chat:write"]),
    m("chat.update", &["chat:write"]),
    session("client.counts"),
    m("conversations.archive", &[]),
    m("conversations.close", &[]),
    m("conversations.create", &[]),
    m("conversations.history", &[]),
    m("conversations.info", &[]),
    m("conversations.invite", &[]),
    m("conversations.join", &["channels:join"]),
    m("conversations.kick", &[]),
    m("conversations.leave", &[]),
    m("conversations.list", &[]),
    m("conversations.mark", &[]),
    m("conversations.members", &[]),
    m("conversations.open", &["im:write", "mpim:write"]),
    m("conversations.rename", &[]),
    m("conversations.replies", &[]),
    m("conversations.setPurpose", &[]),
    m("conversations.setTopic", &[]),
    m("conversations.unarchive", &[]),
    session("emu.apps.deliveries"),
    session("emu.apps.home"),
    session("emu.apps.info"),
    session("emu.apps.install"),
    session("emu.apps.list"),
    session("emu.apps.tokens.create"),
    session("emu.apps.tokens.revoke"),
    session("emu.apps.uninstall"),
    session("emu.apps.verifyUrl"),
    session("emu.blocks.action"),
    session("emu.blocks.suggest"),
    session("emu.inspector.apiLog"),
    session("emu.inspector.deliveries"),
    session("emu.inspector.events"),
    session("emu.commands.list"),
    session("emu.shortcuts.list"),
    session("emu.shortcuts.run"),
    session("emu.views.close"),
    session("emu.views.current"),
    session("emu.views.submit"),
    m("files.completeUploadExternal", &["files:write"]),
    m("files.delete", &["files:write"]),
    m("files.getUploadURLExternal", &["files:write"]),
    m("files.info", &["files:read"]),
    m("files.list", &["files:read"]),
    m("files.upload", &[]),
    m("pins.add", &["pins:write"]),
    m("pins.list", &["pins:read"]),
    m("pins.remove", &["pins:write"]),
    m("reactions.add", &["reactions:write"]),
    m("reactions.get", &["reactions:read"]),
    m("reactions.remove", &["reactions:write"]),
    m("team.info", &["team:read"]),
    m("usergroups.create", &["usergroups:write"]),
    m("usergroups.disable", &["usergroups:write"]),
    m("usergroups.enable", &["usergroups:write"]),
    m("usergroups.list", &["usergroups:read"]),
    m("usergroups.update", &["usergroups:write"]),
    m("usergroups.users.list", &["usergroups:read"]),
    m("usergroups.users.update", &["usergroups:write"]),
    m("users.conversations", &[]),
    m("users.info", &["users:read"]),
    m("users.list", &["users:read"]),
    m("users.lookupByEmail", &["users:read.email"]),
    m("users.profile.get", &["users.profile:read"]),
    m("views.open", &[]),
    m("views.publish", &[]),
    m("views.push", &[]),
    m("views.update", &[]),
];

pub fn lookup(method: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|m| m.name == method)
}

/// Runs a method that [`lookup`] found. `caller` is `Some` whenever the method requires auth.
pub async fn call(state: &AppState, method: &str, caller: Option<Caller>, args: Args) -> ApiResult {
    let spec = lookup(method).expect("dispatch checked the method exists");
    if spec.callers == Callers::Anyone {
        return api_test(args);
    }
    let caller = caller.expect("auth required");
    let allowed = match spec.callers {
        Callers::AppToken => caller.token_kind() == Some(TokenKind::App),
        Callers::Session => caller.is_session(),
        _ => caller.token_kind() != Some(TokenKind::App),
    };
    if !allowed {
        return Err(SlackError::new("not_allowed_token_type").into());
    }
    caller.require_scope(spec.scopes)?;
    let (s, c, a) = (state, &caller, &args);
    match method {
        "apps.connections.open" => apps::connections_open(s, c, a).await,
        "apps.manifest.create" => apps::manifest_create(s, c, a).await,
        "apps.manifest.delete" => apps::manifest_delete(s, c, a).await,
        "apps.manifest.export" => apps::manifest_export(s, c, a).await,
        "apps.manifest.update" => apps::manifest_update(s, c, a).await,
        "apps.manifest.validate" => apps::manifest_validate(s, c, a).await,
        "auth.revoke" => apps::auth_revoke(s, c, a).await,
        "auth.test" => auth_test(s, c),
        "bookmarks.add" => pins::bookmark_add(s, c, a).await,
        "bookmarks.edit" => pins::bookmark_edit(s, c, a).await,
        "bookmarks.list" => pins::bookmark_list(s, c, a).await,
        "bookmarks.remove" => pins::bookmark_remove(s, c, a).await,
        "bots.info" => apps::bots_info(s, c, a).await,
        "chat.command" => interactive::command(s, c, a).await,
        "chat.delete" => chat::delete(s, c, a).await,
        "chat.deleteScheduledMessage" => scheduled::delete(s, c, a).await,
        "chat.getPermalink" => chat::get_permalink(s, c, a).await,
        "chat.meMessage" => chat::me_message(s, c, a).await,
        "chat.postEphemeral" => chat::post_ephemeral(s, c, a).await,
        "chat.postMessage" => chat::post_message(s, c, a).await,
        "chat.scheduleMessage" => scheduled::schedule(s, c, a).await,
        "chat.scheduledMessages.list" => scheduled::list(s, c, a).await,
        "chat.update" => chat::update(s, c, a).await,
        "client.counts" => users::client_counts(s, c, a).await,
        "conversations.archive" => conversations::archive(s, c, a).await,
        "conversations.close" => conversations::close(s, c, a).await,
        "conversations.create" => conversations::create(s, c, a).await,
        "conversations.history" => conversations::history(s, c, a).await,
        "conversations.info" => conversations::info(s, c, a).await,
        "conversations.invite" => conversations::invite(s, c, a).await,
        "conversations.join" => conversations::join(s, c, a).await,
        "conversations.kick" => conversations::kick(s, c, a).await,
        "conversations.leave" => conversations::leave(s, c, a).await,
        "conversations.list" => conversations::list(s, c, a).await,
        "conversations.mark" => conversations::mark(s, c, a).await,
        "conversations.members" => conversations::members(s, c, a).await,
        "conversations.open" => conversations::open(s, c, a).await,
        "conversations.rename" => conversations::rename(s, c, a).await,
        "conversations.replies" => conversations::replies(s, c, a).await,
        "conversations.setPurpose" => conversations::set_purpose(s, c, a).await,
        "conversations.setTopic" => conversations::set_topic(s, c, a).await,
        "conversations.unarchive" => conversations::unarchive(s, c, a).await,
        "emu.apps.deliveries" => apps::emu_deliveries(s, c, a).await,
        "emu.apps.home" => interactive::app_home(s, c, a).await,
        "emu.apps.info" => apps::emu_info(s, c, a).await,
        "emu.apps.install" => apps::emu_install(s, c, a).await,
        "emu.apps.list" => apps::emu_list(s, c, a).await,
        "emu.apps.tokens.create" => apps::emu_token_create(s, c, a).await,
        "emu.apps.tokens.revoke" => apps::emu_token_revoke(s, c, a).await,
        "emu.apps.uninstall" => apps::emu_uninstall(s, c, a).await,
        "emu.apps.verifyUrl" => apps::emu_verify_url(s, c, a).await,
        "emu.blocks.action" => interactive::block_action(s, c, a).await,
        "emu.blocks.suggest" => interactive::block_suggest(s, c, a).await,
        "emu.inspector.apiLog" => inspector::api_log(s, c, a).await,
        "emu.inspector.deliveries" => inspector::deliveries(s, c, a).await,
        "emu.inspector.events" => inspector::events(s, c, a).await,
        "emu.commands.list" => interactive::commands_list(s, c, a).await,
        "emu.shortcuts.list" => interactive::shortcuts_list(s, c, a).await,
        "emu.shortcuts.run" => interactive::shortcut_run(s, c, a).await,
        "emu.views.close" => interactive::view_close(s, c, a).await,
        "emu.views.current" => interactive::views_current(s, c, a).await,
        "emu.views.submit" => interactive::view_submit(s, c, a).await,
        "files.completeUploadExternal" => files::complete_upload(s, c, a).await,
        "files.delete" => files::delete(s, c, a).await,
        "files.getUploadURLExternal" => files::get_upload_url(s, c, a).await,
        "files.info" => files::info(s, c, a).await,
        "files.list" => files::list(s, c, a).await,
        "files.upload" => files::upload_v1(s, c, a).await,
        "pins.add" => pins::add(s, c, a).await,
        "pins.list" => pins::list(s, c, a).await,
        "pins.remove" => pins::remove(s, c, a).await,
        "reactions.add" => reactions::add(s, c, a).await,
        "reactions.get" => reactions::get(s, c, a).await,
        "reactions.remove" => reactions::remove(s, c, a).await,
        "team.info" => users::team_info(s, c, a).await,
        "usergroups.create" => usergroups::create(s, c, a).await,
        "usergroups.disable" => usergroups::disable(s, c, a).await,
        "usergroups.enable" => usergroups::enable(s, c, a).await,
        "usergroups.list" => usergroups::list(s, c, a).await,
        "usergroups.update" => usergroups::update(s, c, a).await,
        "usergroups.users.list" => usergroups::users_list(s, c, a).await,
        "usergroups.users.update" => usergroups::users_update(s, c, a).await,
        "users.conversations" => users::conversations(s, c, a).await,
        "users.info" => users::info(s, c, a).await,
        "users.list" => users::list(s, c, a).await,
        "users.lookupByEmail" => users::lookup_by_email(s, c, a).await,
        "users.profile.get" => users::profile_get(s, c, a).await,
        "views.open" => views::open(s, c, a).await,
        "views.publish" => views::publish(s, c, a).await,
        "views.push" => views::push(s, c, a).await,
        "views.update" => views::update(s, c, a).await,
        _ => unreachable!("method table and match are out of sync: {method}"),
    }
}

fn api_test(mut args: Args) -> ApiResult {
    args.remove("token");
    if let Some(err) = args.get("error").and_then(Value::as_str) {
        return Err(SlackError::new(err).into());
    }
    Ok(json!({"ok": true, "args": args}))
}

fn auth_test(state: &AppState, caller: &Caller) -> ApiResult {
    let mut out = json!({
        "ok": true,
        "url": format!("{}/client/{}/", state.config.public_url, caller.team.id),
        "team": caller.team.name,
        "team_id": caller.team.id,
        "is_enterprise_install": false,
    });
    if let Some(user) = &caller.user {
        out["user"] = json!(user.name);
        out["user_id"] = json!(user.id);
    }
    if let Some(bot_id) = caller.token.as_ref().and_then(|t| t.bot_id.as_ref()) {
        out["bot_id"] = json!(bot_id);
    }
    if let Some(app_id) = caller.token.as_ref().and_then(|t| t.app_id.as_ref()) {
        out["app_id"] = json!(app_id);
    }
    Ok(out)
}
