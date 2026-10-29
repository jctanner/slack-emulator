//! Browser login: accounts, password identities, session cookies, and
//! workspace membership. Bots never use these endpoints.

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::Deserialize;
use serde_json::{Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::ids::{new_secret, sha256_hex};
use slack_emu_core::models::{Account, NewUser, User};
use slack_emu_core::now;
use slack_emu_store::StoreError;

use crate::api::{ApiError, ApiResult, shapes};
use crate::{AppState, services};

pub const SESSION_COOKIE: &str = "slack_emu_session";
pub(crate) const PASSWORD_PROVIDER: &str = "password";
const MIN_PASSWORD_LEN: usize = 8;

pub fn routes() -> axum::Router<AppState> {
    Router::new()
        .route("/signup", post(signup))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/workspaces", get(list_workspaces).post(create_workspace))
        .route("/workspaces/join", post(join_workspace))
}

fn bad_request(code: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, code)
}

fn unauthorized(code: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, code)
}

/// The account behind the request's session cookie, if it is valid.
pub async fn session_account(state: &AppState, jar: &CookieJar) -> Result<Option<Account>, StoreError> {
    match jar.get(SESSION_COOKIE) {
        Some(c) => state.store.session_account(&sha256_hex(c.value())).await,
        None => Ok(None),
    }
}

async fn require_account(state: &AppState, jar: &CookieJar) -> ApiResult<Account> {
    session_account(state, jar)
        .await?
        .ok_or_else(|| unauthorized("not_authed"))
}

pub(crate) async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
    })
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or_else(|| SlackError::internal().into())
}

async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// Starts a session and returns the cookie jar that carries it.
async fn start_session(state: &AppState, jar: CookieJar, account: &Account) -> ApiResult<CookieJar> {
    let secret = new_secret();
    let ttl = state.config.session_ttl_secs;
    state
        .store
        .create_session(&sha256_hex(&secret), &account.id, now() + ttl)
        .await?;
    let cookie = Cookie::build((SESSION_COOKIE, secret))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.config.secure_cookies())
        .max_age(time::Duration::seconds(ttl));
    Ok(jar.add(cookie))
}

/// A workspace handle derived from an email address: `Jane.Doe+x@ex.com` → `jane.doe`.
fn handle_from_email(email: &str) -> String {
    let local = email.split(['@', '+']).next().unwrap_or_default();
    let handle: String = local
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    if handle.is_empty() { "user".into() } else { handle }
}

/// Adds the account to a workspace, picking a free handle. Returns the
/// existing profile if the account is already a member.
pub async fn join(state: &AppState, account: &Account, team_id: &str, owner: bool) -> ApiResult<User> {
    if let Some(user) = state.store.user_for_account(team_id, &account.id).await? {
        return Ok(user);
    }
    let base = handle_from_email(&account.email);
    for n in 1.. {
        let name = if n == 1 { base.clone() } else { format!("{base}{n}") };
        let result = state
            .store
            .create_user(NewUser {
                team_id: team_id.into(),
                account_id: Some(account.id.clone()),
                name,
                real_name: account.display_name.clone(),
                display_name: account.display_name.clone(),
                email: Some(account.email.clone()),
                is_bot: false,
                is_admin: owner,
                is_owner: owner,
            })
            .await;
        match result {
            Err(StoreError::Conflict(_)) if n < 100 => continue,
            other => {
                let user = other?;
                services::on_workspace_join(state, &user).await?;
                return Ok(user);
            }
        }
    }
    unreachable!()
}

async fn me_json(state: &AppState, account: &Account) -> ApiResult {
    let mut workspaces = Vec::new();
    for user in state.store.users_for_account(&account.id).await? {
        if let Some(ws) = state.store.get_workspace(&user.team_id).await? {
            workspaces.push(json!({"team": shapes::team(&ws), "user": shapes::user(&user)}));
        }
    }
    Ok(json!({
        "ok": true,
        "account": {
            "id": account.id,
            "email": account.email,
            "display_name": account.display_name,
        },
        "workspaces": workspaces,
    }))
}

#[derive(Deserialize)]
struct SignupReq {
    email: String,
    password: String,
    display_name: Option<String>,
}

async fn signup(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<SignupReq>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    if !state.config.allow_signup {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "signup_disabled"));
    }
    let email = req.email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(bad_request("invalid_email"));
    }
    if req.password.chars().count() < MIN_PASSWORD_LEN {
        return Err(bad_request("password_too_short"));
    }
    let display_name = req
        .display_name
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| handle_from_email(&email));

    let hash = hash_password(req.password).await?;
    let account = match state
        .store
        .create_account(&email, &display_name, PASSWORD_PROVIDER, &email, Some(&hash))
        .await
    {
        Err(StoreError::Conflict(_)) => {
            return Err(ApiError::new(StatusCode::CONFLICT, "email_taken"));
        }
        other => other?,
    };

    // New accounts land in the default workspace; its first member owns it.
    if let Some(ws) = state
        .store
        .get_workspace_by_domain(&state.config.default_workspace_domain)
        .await?
    {
        let first = state.store.list_users(&ws.id).await?.iter().all(|u| u.is_bot);
        join(&state, &account, &ws.id, first).await?;
    }

    let jar = start_session(&state, jar, &account).await?;
    Ok((jar, Json(me_json(&state, &account).await?)))
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginReq>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let email = req.email.trim().to_ascii_lowercase();
    let found = state.store.find_identity(PASSWORD_PROVIDER, &email).await?;
    let (account, hash) = match found {
        Some(m) => match m.secret_hash {
            Some(h) => (m.account, h),
            None => return Err(unauthorized("invalid_credentials")),
        },
        None => return Err(unauthorized("invalid_credentials")),
    };
    if !verify_password(req.password, hash).await {
        return Err(unauthorized("invalid_credentials"));
    }
    let jar = start_session(&state, jar, &account).await?;
    Ok((jar, Json(me_json(&state, &account).await?)))
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> ApiResult<(CookieJar, Json<Value>)> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        state.store.delete_session(&sha256_hex(c.value())).await?;
    }
    let jar = jar.remove(Cookie::build(SESSION_COOKIE).path("/"));
    Ok((jar, Json(json!({"ok": true}))))
}

async fn me(State(state): State<AppState>, jar: CookieJar) -> ApiResult<Json<Value>> {
    let account = require_account(&state, &jar).await?;
    Ok(Json(me_json(&state, &account).await?))
}

/// Every workspace on the server, flagged with whether the caller belongs to it.
async fn list_workspaces(State(state): State<AppState>, jar: CookieJar) -> ApiResult<Json<Value>> {
    let account = require_account(&state, &jar).await?;
    let mine: Vec<String> = state
        .store
        .users_for_account(&account.id)
        .await?
        .into_iter()
        .map(|u| u.team_id)
        .collect();
    let all: Vec<Value> = state
        .store
        .list_workspaces()
        .await?
        .iter()
        .map(|ws| {
            let mut t = shapes::team(ws);
            t["is_member"] = json!(mine.contains(&ws.id));
            t
        })
        .collect();
    Ok(Json(json!({"ok": true, "workspaces": all})))
}

#[derive(Deserialize)]
struct CreateWorkspaceReq {
    name: String,
    domain: String,
}

pub(crate) fn valid_domain(d: &str) -> bool {
    (1..=21).contains(&d.len())
        && d.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !d.starts_with('-')
        && !d.ends_with('-')
}

async fn create_workspace(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<CreateWorkspaceReq>,
) -> ApiResult<Json<Value>> {
    let account = require_account(&state, &jar).await?;
    let name = req.name.trim();
    let domain = req.domain.trim().to_ascii_lowercase();
    if name.is_empty() {
        return Err(bad_request("invalid_name"));
    }
    if !valid_domain(&domain) {
        return Err(bad_request("invalid_domain"));
    }
    let ws = match state.store.create_workspace(name, &domain).await {
        Err(StoreError::Conflict(_)) => {
            return Err(ApiError::new(StatusCode::CONFLICT, "domain_taken"));
        }
        other => other?,
    };
    services::ensure_workspace_defaults(&state, &ws.id).await?;
    let user = join(&state, &account, &ws.id, true).await?;
    Ok(Json(
        json!({"ok": true, "team": shapes::team(&ws), "user": shapes::user(&user)}),
    ))
}

#[derive(Deserialize)]
struct JoinReq {
    team_id: String,
}

async fn join_workspace(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<JoinReq>,
) -> ApiResult<Json<Value>> {
    let account = require_account(&state, &jar).await?;
    let ws = state
        .store
        .get_workspace(&req.team_id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "team_not_found"))?;
    let user = join(&state, &account, &ws.id, false).await?;
    Ok(Json(
        json!({"ok": true, "team": shapes::team(&ws), "user": shapes::user(&user)}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles() {
        assert_eq!(handle_from_email("Jane.Doe+tag@example.com"), "jane.doe");
        assert_eq!(handle_from_email("!!!@example.com"), "user");
    }

    #[test]
    fn domains() {
        assert!(valid_domain("acme-corp"));
        assert!(!valid_domain("Acme"));
        assert!(!valid_domain("-acme"));
        assert!(!valid_domain(""));
    }
}
