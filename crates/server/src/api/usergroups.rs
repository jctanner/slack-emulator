//! User groups (`usergroups.*`): named sets of people mentioned together as
//! `<!subteam^S…|@handle>`.

use serde_json::{Value, json};
use slack_emu_core::ids::{IdKind, new_id};
use slack_emu_core::models::Usergroup;
use slack_emu_core::now;
use slack_emu_store::StoreError;

use super::args::{self, bool_arg, opt_str, req_str};
use super::conversations::{fail, team_user};
use super::{ApiResult, Args, Caller};
use crate::AppState;
use crate::events::{Audience, publish};

/// The usergroup object Slack returns.
fn group_json(g: &Usergroup, users: Option<&[String]>) -> Value {
    let mut v = json!({
        "id": g.id,
        "team_id": g.team_id,
        "is_usergroup": true,
        "is_subteam": true,
        "name": g.name,
        "description": g.description,
        "handle": g.handle,
        "is_external": false,
        "date_create": g.created,
        "date_update": g.updated,
        "date_delete": g.disabled_at.unwrap_or(0),
        "auto_type": null,
        "auto_provision": false,
        "enterprise_subteam_id": "",
        "created_by": g.created_by,
        "updated_by": g.updated_by,
        "deleted_by": g.deleted_by,
        "prefs": {"channels": g.channels, "groups": []},
        "channel_count": g.channels.len(),
    });
    if let Some(users) = users {
        v["users"] = json!(users);
        v["user_count"] = json!(users.len());
    }
    v
}

fn valid_handle(h: &str) -> bool {
    (1..=21).contains(&h.chars().count())
        && h.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// A handle from a name: "Platform Oncall" → `platform-oncall`.
fn handle_from(name: &str) -> String {
    let h: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(21)
        .collect();
    if h.is_empty() { "group".into() } else { h }
}

/// Handles share a namespace with people's handles and special mentions.
async fn check_names(state: &AppState, caller: &Caller, g: &Usergroup) -> ApiResult<()> {
    if !valid_handle(&g.handle) || matches!(g.handle.as_str(), "here" | "channel" | "everyone") {
        return fail("invalid_handle");
    }
    if g.name.trim().is_empty() {
        return fail("invalid_name");
    }
    for other in state.store.list_usergroups(&caller.team.id).await? {
        if other.id == g.id {
            continue;
        }
        if other.name == g.name {
            return fail("name_already_exists");
        }
        if other.handle == g.handle {
            return fail("handle_already_exists");
        }
    }
    if state
        .store
        .list_users(&caller.team.id)
        .await?
        .iter()
        .any(|u| u.name == g.handle)
    {
        return fail("handle_already_exists");
    }
    Ok(())
}

async fn load(state: &AppState, caller: &Caller, a: &Args) -> ApiResult<Usergroup> {
    let id = req_str(a, "usergroup")?;
    match state.store.get_usergroup(&id).await? {
        Some(g) if g.team_id == caller.team.id => Ok(g),
        _ => fail("no_such_subteam"),
    }
}

async fn save(state: &AppState, g: &Usergroup) -> ApiResult<()> {
    match state.store.update_usergroup(g).await {
        Err(StoreError::Conflict(_)) => fail("name_already_exists"),
        other => Ok(other?),
    }
}

/// `subteam_*` events go to the whole workspace.
fn announce(state: &AppState, team_id: &str, event: Value) {
    publish(state, team_id, Audience::Team, false, event);
}

async fn with_users(state: &AppState, g: &Usergroup, include: bool) -> ApiResult<Value> {
    let users = if include {
        Some(state.store.usergroup_members(&g.id).await?)
    } else {
        None
    };
    Ok(group_json(g, users.as_deref()))
}

pub async fn create(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let me = caller.user()?;
    let name = req_str(a, "name")?;
    let now = now();
    let g = Usergroup {
        id: new_id(IdKind::Usergroup),
        team_id: caller.team.id.clone(),
        handle: opt_str(a, "handle").unwrap_or_else(|| handle_from(&name)),
        name,
        description: opt_str(a, "description").unwrap_or_default(),
        channels: args::list(a, "channels"),
        created_by: me.id.clone(),
        updated_by: me.id.clone(),
        created: now,
        updated: now,
        disabled_at: None,
        deleted_by: None,
    };
    check_names(state, caller, &g).await?;
    match state.store.create_usergroup(&g).await {
        Err(StoreError::Conflict(_)) => return fail("name_already_exists"),
        other => other?,
    }
    let json = group_json(&g, Some(&[]));
    announce(state, &g.team_id, json!({"type": "subteam_created", "subteam": json}));
    Ok(json!({"ok": true, "usergroup": json}))
}

pub async fn list(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let include_disabled = bool_arg(a, "include_disabled", false);
    let include_users = bool_arg(a, "include_users", false);
    let mut out = Vec::new();
    for g in state.store.list_usergroups(&caller.team.id).await? {
        if g.disabled_at.is_some() && !include_disabled {
            continue;
        }
        let mut v = with_users(state, &g, include_users).await?;
        if bool_arg(a, "include_count", false) && !include_users {
            v["user_count"] = json!(state.store.usergroup_members(&g.id).await?.len());
        }
        out.push(v);
    }
    Ok(json!({"ok": true, "usergroups": out}))
}

pub async fn update(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let mut g = load(state, caller, a).await?;
    if let Some(name) = opt_str(a, "name") {
        g.name = name;
    }
    if let Some(handle) = opt_str(a, "handle") {
        g.handle = handle;
    }
    if let Some(d) = a.get("description").and_then(Value::as_str) {
        g.description = d.to_owned();
    }
    if a.contains_key("channels") {
        g.channels = args::list(a, "channels");
    }
    check_names(state, caller, &g).await?;
    g.updated = now();
    g.updated_by = caller.user()?.id.clone();
    save(state, &g).await?;
    let json = with_users(state, &g, true).await?;
    announce(state, &g.team_id, json!({"type": "subteam_updated", "subteam": json}));
    Ok(json!({"ok": true, "usergroup": with_users(state, &g, bool_arg(a, "include_users", false)).await?}))
}

async fn set_disabled(state: &AppState, caller: &Caller, a: &Args, disable: bool) -> ApiResult {
    let mut g = load(state, caller, a).await?;
    let me = caller.user()?;
    match (disable, g.disabled_at) {
        (true, Some(_)) => return fail("already_disabled"),
        (false, None) => return fail("already_enabled"),
        _ => {}
    }
    g.disabled_at = disable.then(now);
    g.deleted_by = disable.then(|| me.id.clone());
    g.updated = now();
    g.updated_by = me.id.clone();
    save(state, &g).await?;
    let json = with_users(state, &g, true).await?;
    announce(state, &g.team_id, json!({"type": "subteam_updated", "subteam": json}));
    Ok(json!({"ok": true, "usergroup": json}))
}

pub async fn disable(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    set_disabled(state, caller, a, true).await
}

pub async fn enable(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    set_disabled(state, caller, a, false).await
}

pub async fn users_list(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let g = load(state, caller, a).await?;
    if g.disabled_at.is_some() && !bool_arg(a, "include_disabled", false) {
        return fail("no_such_subteam");
    }
    Ok(json!({"ok": true, "users": state.store.usergroup_members(&g.id).await?}))
}

pub async fn users_update(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let mut g = load(state, caller, a).await?;
    let wanted = args::list(a, "users");
    if wanted.is_empty() {
        return fail("no_users_provided");
    }
    let mut users = Vec::new();
    for id in &wanted {
        let u = team_user(state, caller, id)
            .await
            .map_err(|_| slack_emu_core::SlackError::new("invalid_users"))?;
        if !users.contains(&u.id) {
            users.push(u.id);
        }
    }
    let before = state.store.usergroup_members(&g.id).await?;
    state.store.set_usergroup_members(&g.id, &users).await?;
    let previous_update = g.updated;
    g.updated = now();
    g.updated_by = caller.user()?.id.clone();
    save(state, &g).await?;

    let added: Vec<&String> = users.iter().filter(|u| !before.contains(u)).collect();
    let removed: Vec<&String> = before.iter().filter(|u| !users.contains(u)).collect();
    announce(
        state,
        &g.team_id,
        json!({
            "type": "subteam_members_changed",
            "subteam_id": g.id,
            "team_id": g.team_id,
            "date_previous_update": previous_update,
            "date_update": g.updated,
            "added_users": added,
            "added_users_count": added.len().to_string(),
            "removed_users": removed,
            "removed_users_count": removed.len().to_string(),
        }),
    );
    for (who, kind) in added
        .iter()
        .map(|u| (u, "subteam_self_added"))
        .chain(removed.iter().map(|u| (u, "subteam_self_removed")))
    {
        publish(
            state,
            &g.team_id,
            Audience::Users(vec![(*who).clone()]),
            false,
            json!({"type": kind, "subteam_id": g.id}),
        );
    }
    let mut sorted = users.clone();
    sorted.sort();
    Ok(json!({"ok": true, "usergroup": group_json(&g, bool_arg(a, "include_users", true).then_some(&sorted[..]))}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles() {
        assert_eq!(handle_from("Platform Oncall"), "platform-oncall");
        assert!(valid_handle("sre.oncall"));
        assert!(!valid_handle("Has Caps"));
    }
}
