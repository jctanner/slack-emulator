//! Apps: Slack's manifest API (`apps.manifest.*`), installation and token
//! management for the web UI (`emu.apps.*`), Socket Mode (`apps.connections.open`),
//! `bots.info` and `auth.revoke`.

use serde_json::{Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::ids::{IdKind, TokenKind, new_api_token, new_id, new_secret};
use slack_emu_core::manifest::{Manifest, ManifestError};
use slack_emu_core::models::{ApiToken, App, NewUser, User};
use slack_emu_core::now;
use slack_emu_store::StoreError;

use super::args::{self, opt_str, req_str};
use super::conversations::fail;
use super::{ApiError, ApiResult, Args, Caller};
use crate::AppState;
use crate::delivery::event_callback;
use crate::events::{Audience, publish};

fn invalid_manifest(errors: Vec<ManifestError>) -> ApiError {
    ApiError::from(SlackError::new("invalid_manifest")).with("errors", json!(errors))
}

/// Parses and validates the `manifest` argument (a JSON string or object).
pub(crate) fn manifest_arg(args: &Args) -> ApiResult<(Value, Manifest)> {
    let raw =
        args::json(args, "manifest")?.ok_or_else(|| ApiError::invalid_arguments("missing required field: manifest"))?;
    let manifest: Manifest = serde_json::from_value(raw.clone()).map_err(|e| {
        invalid_manifest(vec![ManifestError {
            message: format!("Couldn't parse the manifest: {e}"),
            pointer: "/".into(),
        }])
    })?;
    let errors = manifest.validate();
    if !errors.is_empty() {
        return Err(invalid_manifest(errors));
    }
    Ok((raw, manifest))
}

/// An app in the caller's workspace that the caller may manage (its creator,
/// or a workspace admin/owner).
async fn managed_app(state: &AppState, caller: &Caller, args: &Args) -> ApiResult<App> {
    let id = req_str(args, "app_id")?;
    let app = match state.store.get_app(&id).await? {
        Some(a) if a.team_id == caller.team.id => a,
        _ => return fail("invalid_app_id"),
    };
    let me = caller.user()?;
    if app.creator.as_deref() != Some(&me.id) && !me.is_admin && !me.is_owner {
        return fail("not_allowed");
    }
    Ok(app)
}

fn random_digits(n: usize) -> String {
    new_secret()
        .bytes()
        .map(|b| char::from(b'0' + b % 10))
        .take(n)
        .collect()
}

/// Verifies the events request URL if it changed. Socket Mode apps don't need one.
async fn verify_request_url(state: &AppState, app: &mut App) -> Option<String> {
    let manifest = app.manifest();
    let url = manifest.request_url()?.to_owned();
    if manifest.settings.socket_mode_enabled || app.verified_request_url.as_deref() == Some(&url) {
        return None;
    }
    match state.delivery.verify_url(app, &url).await {
        Ok(()) => {
            app.verified_request_url = Some(url);
            None
        }
        Err(e) => Some(e),
    }
}

pub async fn manifest_validate(_state: &AppState, _caller: &Caller, args: &Args) -> ApiResult {
    manifest_arg(args)?;
    Ok(json!({"ok": true}))
}

/// Credentials a fixture may fix in advance (`/_emu/seed`); the rest are random.
#[derive(Debug, Default)]
pub(crate) struct FixedCredentials {
    pub app_id: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub signing_secret: Option<String>,
    pub verification_token: Option<String>,
}

/// Creates an app from a manifest argument. Returns the app and, if its
/// request URL failed verification, why.
pub(crate) async fn create_app(
    state: &AppState,
    team_id: &str,
    creator: &str,
    args: &Args,
    fixed: FixedCredentials,
) -> ApiResult<(App, Option<String>)> {
    let (raw, manifest) = manifest_arg(args)?;
    let mut app = App {
        id: fixed.app_id.unwrap_or_else(|| new_id(IdKind::App)),
        team_id: team_id.into(),
        name: manifest.display_information.name.trim().to_owned(),
        creator: Some(creator.into()),
        created: now(),
        updated: now(),
        manifest_json: raw,
        client_id: fixed
            .client_id
            .unwrap_or_else(|| format!("{}.{}", random_digits(12), random_digits(13))),
        client_secret: fixed.client_secret.unwrap_or_else(hex_secret),
        signing_secret: fixed.signing_secret.unwrap_or_else(hex_secret),
        verification_token: fixed
            .verification_token
            .unwrap_or_else(|| new_secret()[..24].to_owned()),
        bot_user_id: None,
        bot_id: None,
        installed_by: None,
        installed_at: None,
        verified_request_url: None,
    };
    if state.store.get_app(&app.id).await?.is_some() {
        return fail("app_already_exists");
    }
    let url_error = verify_request_url(state, &mut app).await;
    state.store.create_app(&app).await?;
    Ok((app, url_error))
}

pub async fn manifest_create(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?;
    let (app, url_error) = create_app(state, &caller.team.id, &me.id, args, FixedCredentials::default()).await?;
    let mut out = json!({
        "ok": true,
        "app_id": app.id,
        "credentials": credentials(&app),
        "oauth_authorize_url": format!("{}/oauth/v2/authorize?client_id={}", state.config.public_url, app.client_id),
    });
    if let Some(e) = url_error {
        out["warning"] = json!("request_url_not_verified");
        out["response_metadata"] = json!({"warnings": ["request_url_not_verified"], "messages": [e]});
    }
    Ok(out)
}

fn hex_secret() -> String {
    hex::encode(new_secret().as_bytes())[..32].to_owned()
}

pub(crate) fn credentials(app: &App) -> Value {
    json!({
        "client_id": app.client_id,
        "client_secret": app.client_secret,
        "verification_token": app.verification_token,
        "signing_secret": app.signing_secret,
    })
}

pub async fn manifest_update(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let mut app = managed_app(state, caller, args).await?;
    let (raw, manifest) = manifest_arg(args)?;
    let before = app.manifest();
    let permissions_updated = before.oauth_config.scopes != manifest.oauth_config.scopes;
    app.name = manifest.display_information.name.trim().to_owned();
    app.manifest_json = raw;
    app.updated = now();
    let url_error = verify_request_url(state, &mut app).await;
    state.store.update_app(&app).await?;
    let mut out = json!({"ok": true, "app_id": app.id, "permissions_updated": permissions_updated});
    if let Some(e) = url_error {
        out["warning"] = json!("request_url_not_verified");
        out["response_metadata"] = json!({"warnings": ["request_url_not_verified"], "messages": [e]});
    }
    Ok(out)
}

pub async fn manifest_export(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    Ok(json!({"ok": true, "manifest": app.manifest_json}))
}

pub async fn manifest_delete(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    if app.is_installed() {
        uninstall(state, &app).await?;
    }
    state.store.delete_app(&app.id).await?;
    Ok(json!({"ok": true}))
}

/// A short workspace handle for a bot: "Echo Bot" → `echo-bot`.
fn bot_handle(display_name: &str) -> String {
    let h: String = display_name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if h.is_empty() { "bot".into() } else { h }
}

/// Creates (or revives and renames) the app's bot user.
async fn ensure_bot_user(state: &AppState, app: &mut App, display_name: &str) -> ApiResult<User> {
    if let Some(id) = &app.bot_user_id
        && let Some(mut user) = state.store.get_user(id).await?
    {
        user.deleted = false;
        user.real_name = display_name.into();
        user.display_name = display_name.into();
        state.store.update_user(&user).await?;
        return Ok(user);
    }
    let base = bot_handle(display_name);
    for n in 1..100 {
        let name = if n == 1 { base.clone() } else { format!("{base}{n}") };
        let result = state
            .store
            .create_user(NewUser {
                team_id: app.team_id.clone(),
                account_id: None,
                name,
                real_name: display_name.into(),
                display_name: display_name.into(),
                email: None,
                is_bot: true,
                is_admin: false,
                is_owner: false,
            })
            .await;
        match result {
            Err(StoreError::Conflict(_)) => continue,
            other => {
                let user = other?;
                app.bot_user_id = Some(user.id.clone());
                return Ok(user);
            }
        }
    }
    fail("internal_error")
}

/// Issues or updates the token of `kind` for (app, user), returning it.
async fn upsert_token(
    state: &AppState,
    app: &App,
    kind: TokenKind,
    user_id: &str,
    scopes: &[String],
) -> ApiResult<String> {
    let existing = state
        .store
        .app_tokens(&app.id)
        .await?
        .into_iter()
        .find(|t| t.kind == kind && !t.revoked && t.user_id.as_deref() == Some(user_id));
    if let Some(t) = existing {
        state.store.set_token_scopes(&t.token, scopes).await?;
        return Ok(t.token);
    }
    let token = ApiToken {
        token: new_api_token(kind),
        kind,
        team_id: app.team_id.clone(),
        user_id: Some(user_id.into()),
        app_id: Some(app.id.clone()),
        bot_id: if kind == TokenKind::Bot {
            app.bot_id.clone()
        } else {
            None
        },
        scopes: scopes.to_vec(),
        created: now(),
        revoked: false,
        name: None,
    };
    state.store.create_token(&token).await?;
    Ok(token.token)
}

/// Installs (or reinstalls) the app into its workspace. Returns the same shape
/// as `oauth.v2.access`.
pub async fn emu_install(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let mut app = managed_app(state, caller, args).await?;
    let manifest = app.manifest();
    let me = caller.user()?.clone();
    let mut out = json!({
        "ok": true,
        "app_id": app.id,
        "team": {"id": app.team_id, "name": caller.team.name},
        "enterprise": null,
        "is_enterprise_install": false,
        "authed_user": {"id": me.id},
    });

    if let Some(bot) = &manifest.features.bot_user {
        if app.bot_id.is_none() {
            app.bot_id = Some(new_id(IdKind::Bot));
        }
        let bot_user = ensure_bot_user(state, &mut app, &bot.display_name).await?;
        let token = upsert_token(state, &app, TokenKind::Bot, &bot_user.id, manifest.bot_scopes()).await?;
        out["access_token"] = json!(token);
        out["token_type"] = json!("bot");
        out["scope"] = json!(manifest.bot_scopes().join(","));
        out["bot_user_id"] = json!(bot_user.id);
    }
    let user_scopes = &manifest.oauth_config.scopes.user;
    if !user_scopes.is_empty() {
        let token = upsert_token(state, &app, TokenKind::User, &me.id, user_scopes).await?;
        out["authed_user"] = json!({
            "id": me.id,
            "scope": user_scopes.join(","),
            "access_token": token,
            "token_type": "user",
        });
    }
    app.installed_by = Some(me.id.clone());
    app.installed_at = Some(now());
    app.updated = now();
    state.store.update_app(&app).await?;
    announce_bot_user(state, &app).await?;
    Ok(out)
}

/// Revokes the app's bot and user tokens and deactivates its bot user.
async fn uninstall(state: &AppState, app: &App) -> ApiResult<()> {
    // Tell the app first, while it can still be reached.
    if app.manifest().bot_events().iter().any(|e| e == "app_uninstalled") {
        let ev = json!({"type": "app_uninstalled", "event_ts": state.clock.next()});
        state.delivery.enqueue_event(state, &app.id, event_callback(app, ev));
    }
    for t in state.store.app_tokens(&app.id).await? {
        if t.kind != TokenKind::App && !t.revoked {
            state.store.revoke_token(&t.token).await?;
        }
    }
    if let Some(id) = &app.bot_user_id
        && let Some(mut user) = state.store.get_user(id).await?
    {
        user.deleted = true;
        state.store.update_user(&user).await?;
    }
    let mut app = app.clone();
    app.installed_at = None;
    app.installed_by = None;
    app.updated = now();
    state.store.update_app(&app).await?;
    announce_bot_user(state, &app).await?;
    state.delivery.sockets.disconnect_all(&app.id, "link_disabled");
    Ok(())
}

/// Tells the workspace the bot user was created or changed (`user_change`).
async fn announce_bot_user(state: &AppState, app: &App) -> ApiResult<()> {
    if let Some(id) = &app.bot_user_id
        && let Some(user) = state.store.get_user(id).await?
    {
        publish(
            state,
            &app.team_id,
            Audience::Team,
            false,
            json!({"type": "user_change", "user": super::shapes::user_with_app(&user, Some(app))}),
        );
    }
    Ok(())
}

pub async fn emu_uninstall(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    if !app.is_installed() {
        return fail("app_not_installed");
    }
    uninstall(state, &app).await?;
    Ok(json!({"ok": true}))
}

async fn summary(state: &AppState, app: &App) -> ApiResult<Value> {
    let manifest = app.manifest();
    let tokens = state.store.app_tokens(&app.id).await?;
    let bot_token = tokens.iter().find(|t| t.kind == TokenKind::Bot && !t.revoked);
    // Scopes changed since the last install: a reinstall is needed to apply them.
    let needs_reinstall = app.is_installed()
        && bot_token.is_some_and(|t| {
            let mut a = t.scopes.clone();
            let mut b = manifest.bot_scopes().to_vec();
            a.sort();
            b.sort();
            a != b
        });
    let request_url = manifest.request_url().map(str::to_owned);
    Ok(json!({
        "id": app.id,
        "name": app.name,
        "description": manifest.display_information.description,
        "creator": app.creator,
        "created": app.created,
        "updated": app.updated,
        "installed": app.is_installed(),
        "installed_by": app.installed_by,
        "needs_reinstall": needs_reinstall,
        "bot_user_id": app.bot_user_id,
        "bot_id": app.bot_id,
        "socket_mode": manifest.settings.socket_mode_enabled,
        "socket_connections": state.delivery.sockets.connection_count(&app.id),
        "request_url": request_url,
        "request_url_verified": request_url.is_some() && app.verified_request_url == request_url,
        "bot_events": manifest.bot_events(),
        "bot_scopes": manifest.bot_scopes(),
        "interactivity": manifest.interactivity_enabled(),
        "interactivity_url": manifest.interactivity_url(),
        "slash_commands": manifest.features.slash_commands.iter().map(|c| json!({
            "command": c.command,
            "description": c.description,
            "url": c.url,
        })).collect::<Vec<_>>(),
        "home_tab_enabled": manifest.home_tab_enabled(),
    }))
}

pub async fn emu_list(state: &AppState, caller: &Caller, _args: &Args) -> ApiResult {
    let mut apps = Vec::new();
    for app in state.store.list_apps(&caller.team.id).await? {
        apps.push(summary(state, &app).await?);
    }
    Ok(json!({"ok": true, "apps": apps}))
}

pub async fn emu_info(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    let mut info = summary(state, &app).await?;
    info["credentials"] = credentials(&app);
    info["manifest"] = app.manifest_json.clone();
    info["tokens"] = state
        .store
        .app_tokens(&app.id)
        .await?
        .into_iter()
        .filter(|t| !t.revoked)
        .map(|t| {
            json!({
                "token": t.token,
                "type": t.kind.as_str(),
                "user_id": t.user_id,
                "scopes": t.scopes,
                "name": t.name,
                "created": t.created,
            })
        })
        .collect();
    Ok(json!({"ok": true, "app": info}))
}

pub async fn emu_token_create(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    let mut scopes = args::list(args, "scopes");
    if scopes.is_empty() {
        scopes.push("connections:write".into());
    }
    if let Some(bad) = scopes.iter().find(|s| {
        !matches!(
            s.as_str(),
            "connections:write" | "authorizations:read" | "app_configurations:write"
        )
    }) {
        return Err(ApiError::invalid_arguments(format!("unknown app-level scope: {bad}")));
    }
    let token = ApiToken {
        token: new_api_token(TokenKind::App),
        kind: TokenKind::App,
        team_id: app.team_id.clone(),
        user_id: None,
        app_id: Some(app.id.clone()),
        bot_id: None,
        scopes,
        created: now(),
        revoked: false,
        name: Some(opt_str(args, "name").unwrap_or_else(|| "app-token".into())),
    };
    state.store.create_token(&token).await?;
    Ok(json!({"ok": true, "token": token.token, "scopes": token.scopes, "name": token.name}))
}

pub async fn emu_token_revoke(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    // Not `token`: that argument authenticates the caller.
    let token = req_str(args, "target")?;
    match state.store.get_token(&token).await? {
        Some(t) if t.app_id.as_deref() == Some(&app.id) => {
            state.store.revoke_token(&t.token).await?;
            if t.kind == TokenKind::App {
                state.delivery.sockets.disconnect_all(&app.id, "link_disabled");
            }
            Ok(json!({"ok": true}))
        }
        _ => fail("token_not_found"),
    }
}

pub async fn emu_verify_url(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let mut app = managed_app(state, caller, args).await?;
    let Some(url) = app.manifest().request_url().map(str::to_owned) else {
        return fail("no_request_url");
    };
    match state.delivery.verify_url(&app, &url).await {
        Ok(()) => {
            app.verified_request_url = Some(url);
            state.store.update_app(&app).await?;
            Ok(json!({"ok": true, "verified": true}))
        }
        Err(e) => Ok(json!({"ok": true, "verified": false, "message": e})),
    }
}

pub async fn emu_deliveries(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = managed_app(state, caller, args).await?;
    Ok(json!({"ok": true, "deliveries": state.delivery.deliveries(&app.id)}))
}

pub async fn connections_open(state: &AppState, caller: &Caller, _args: &Args) -> ApiResult {
    let app_id = caller
        .token
        .as_ref()
        .and_then(|t| t.app_id.clone())
        .ok_or_else(SlackError::invalid_auth)?;
    if state.store.get_app(&app_id).await?.is_none() {
        return fail("invalid_auth");
    }
    let ticket = state.delivery.sockets.issue_ticket(&app_id);
    let base = state
        .config
        .public_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    Ok(json!({"ok": true, "url": format!("{base}/ws/socket-mode?ticket={ticket}")}))
}

pub async fn bots_info(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let bot_id = opt_str(args, "bot").ok_or_else(|| SlackError::new("bot_not_found"))?;
    match state.store.app_by_bot_id(&bot_id).await? {
        Some(app) if app.team_id == caller.team.id => Ok(json!({"ok": true, "bot": bot_profile(&app, false)})),
        _ => fail("bot_not_found"),
    }
}

/// The `bot_profile` / `bots.info` object for an app's bot.
pub fn bot_profile(app: &App, with_team: bool) -> Value {
    let mut v = json!({
        "id": app.bot_id,
        "deleted": !app.is_installed(),
        "name": app.manifest().features.bot_user.map(|b| b.display_name).unwrap_or_else(|| app.name.clone()),
        "updated": app.updated,
        "app_id": app.id,
        "user_id": app.bot_user_id,
        "icons": {"image_36": "", "image_48": "", "image_72": ""},
    });
    if with_team {
        v["team_id"] = json!(app.team_id);
    }
    v
}

pub async fn auth_revoke(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let Some(token) = &caller.token else {
        return fail("not_allowed_token_type");
    };
    if !args::bool_arg(args, "test", false) {
        state.store.revoke_token(&token.token).await?;
    }
    Ok(json!({"ok": true, "revoked": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles() {
        assert_eq!(bot_handle("Echo Bot"), "echo-bot");
        assert_eq!(bot_handle("  !!! "), "bot");
    }
}
