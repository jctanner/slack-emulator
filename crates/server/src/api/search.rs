//! `search.messages`, `search.files` and `search.all`, with Slack's query
//! modifiers: `in:#channel`, `in:@person`, `from:@person` (or `from:me`),
//! `before:`, `after:` and `on:` (YYYY-MM-DD, `today`, `yesterday`),
//! `has:link`, `has:pin`, `is:thread`, and "quoted phrases".
//!
//! Like Slack, search needs a person (or a user token with `search:read`);
//! bot tokens get `not_allowed_token_type` (see `Callers::People`). Results are ordered by time ("score" sorts newest
//! first too, since there's no relevance ranking), and terms match as
//! case-insensitive substrings.

use serde_json::{Value, json};
use slack_emu_core::models::{Conversation, Message};
use slack_emu_core::now;
use slack_emu_store::MessageSearch;

use super::args::{opt_str, req_str};
use super::conversations::find_conversation;
use super::files::{file_object, visible_files};
use super::{ApiResult, Args, Caller, shapes};
use crate::AppState;

/// A parsed query.
#[derive(Debug, Default, PartialEq)]
struct Query {
    terms: Vec<String>,
    in_: Option<String>,
    from: Option<String>,
    before: Option<String>,
    after: Option<String>,
    on: Option<String>,
    has: Vec<String>,
    is: Vec<String>,
}

/// Splits on spaces, keeping "quoted phrases" together.
fn tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in q.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse(q: &str) -> Query {
    let mut out = Query::default();
    for t in tokens(q) {
        let lower = t.to_lowercase();
        let (key, value) = lower.split_once(':').unwrap_or(("", ""));
        // Keep the original case of names in values.
        let raw = t.split_once(':').map(|(_, v)| v.to_owned()).unwrap_or_default();
        match key {
            "in" if !value.is_empty() => out.in_ = Some(raw),
            "from" if !value.is_empty() => out.from = Some(raw),
            "before" => out.before = Some(value.into()),
            "after" => out.after = Some(value.into()),
            "on" | "during" => out.on = Some(value.into()),
            "has" => out.has.push(value.into()),
            "is" => out.is.push(value.into()),
            _ => out.terms.push(t),
        }
    }
    out
}

/// Midnight UTC of a day (`YYYY-MM-DD`, `today` or `yesterday`), as Unix seconds.
fn day_start(day: &str) -> Option<i64> {
    let today = now() - now().rem_euclid(86_400);
    match day {
        "today" => return Some(today),
        "yesterday" => return Some(today - 86_400),
        _ => {}
    }
    let mut parts = day.splitn(3, '-').map(|p| p.parse::<i32>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    let month = time::Month::try_from(u8::try_from(m).ok()?).ok()?;
    let date = time::Date::from_calendar_date(y, month, u8::try_from(d).ok()?).ok()?;
    Some(date.midnight().assume_utc().unix_timestamp())
}

fn ts_of(secs: i64) -> String {
    format!("{secs:010}.000000")
}

/// `[after, before)` in ts terms. `after:` starts the day after, as on Slack.
fn ts_bounds(q: &Query) -> (Option<String>, Option<String>) {
    let mut after = q.after.as_deref().and_then(day_start).map(|d| d + 86_400);
    let mut before = q.before.as_deref().and_then(day_start);
    if let Some(on) = q.on.as_deref().and_then(day_start) {
        after = Some(after.map_or(on, |a| a.max(on)));
        before = Some(before.map_or(on + 86_400, |b| b.min(on + 86_400)));
    }
    (after.map(ts_of), before.map(ts_of))
}

/// `from:` → a user ID in the caller's workspace.
async fn resolve_user(state: &AppState, caller: &Caller, raw: &str) -> ApiResult<Option<String>> {
    let me = caller.user()?;
    let v = raw.trim_start_matches('@');
    if v.eq_ignore_ascii_case("me") {
        return Ok(Some(me.id.clone()));
    }
    let id = v
        .trim_start_matches("<@")
        .trim_end_matches('>')
        .split('|')
        .next()
        .unwrap_or(v);
    let users = state.store.list_users(&caller.team.id).await?;
    Ok(users
        .iter()
        .find(|u| u.id == id || u.name.eq_ignore_ascii_case(v))
        .map(|u| u.id.clone()))
}

/// `in:` → a conversation: `#name`, a channel ID, or `@person` for your DM with them.
async fn resolve_in(state: &AppState, caller: &Caller, raw: &str) -> ApiResult<Option<Conversation>> {
    if raw.starts_with('@') || raw.starts_with("<@") {
        let Some(other) = resolve_user(state, caller, raw).await? else {
            return Ok(None);
        };
        let me = caller.user()?;
        let key = Conversation::dm_key_for(&[me.id.clone(), other]);
        return Ok(state.store.conversation_by_dm_key(&caller.team.id, &key).await?);
    }
    let name = raw
        .trim_start_matches("<#")
        .trim_end_matches('>')
        .split('|')
        .next()
        .unwrap_or(raw)
        .trim_start_matches('#');
    Ok(find_conversation(state, caller, name).await.ok())
}

/// Shared paging: `count` (default 20, max 100) and `page` (from 1).
fn paging(a: &Args, total: usize) -> (usize, usize, Value, Value) {
    let count = opt_str(a, "count")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(20)
        .min(100);
    let pages = total.div_ceil(count).max(1);
    let page = opt_str(a, "page")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
        .min(pages);
    let first = (page - 1) * count + 1;
    let last = (page * count).min(total);
    (
        count,
        page,
        json!({"total_count": total, "page": page, "per_page": count, "page_count": pages, "first": first, "last": last}),
        json!({"count": count, "total": total, "page": page, "pages": pages}),
    )
}

fn newest_first(a: &Args) -> bool {
    opt_str(a, "sort_dir").is_none_or(|d| d != "asc")
}

/// The `channel` object in a search match.
fn match_channel(conv: &Conversation, viewer: &str) -> Value {
    json!({
        "id": conv.id,
        "is_channel": conv.is_channel(),
        "is_group": conv.is_private && !conv.is_im && !conv.is_mpim,
        "is_im": conv.is_im,
        "is_mpim": conv.is_mpim,
        "is_private": conv.is_private || conv.is_im || conv.is_mpim,
        "is_shared": false,
        "is_org_shared": false,
        "is_ext_shared": false,
        "is_pending_ext_shared": false,
        "pending_shared": [],
        "name": if conv.is_im { shapes::im_peer(conv, Some(viewer)).unwrap_or_default() } else { conv.name.clone().unwrap_or_default() },
    })
}

async fn message_results(state: &AppState, caller: &Caller, a: &Args) -> ApiResult<Value> {
    let me = caller.user()?;
    let q = parse(&req_str(a, "query")?);
    let empty = |a: &Args| {
        let (_, _, pagination, paging) = paging(a, 0);
        json!({"total": 0, "pagination": pagination, "paging": paging, "matches": []})
    };
    let channel = match &q.in_ {
        Some(raw) => match resolve_in(state, caller, raw).await? {
            Some(c) => Some(c.id),
            None => return Ok(empty(a)),
        },
        None => None,
    };
    let user = match &q.from {
        Some(raw) => match resolve_user(state, caller, raw).await? {
            Some(u) => Some(u),
            None => return Ok(empty(a)),
        },
        None => None,
    };
    let (after, before) = ts_bounds(&q);
    let search = MessageSearch {
        team_id: &caller.team.id,
        viewer: &me.id,
        terms: q.terms.clone(),
        channel_id: channel,
        user_id: user,
        after,
        before,
        limit: 5000,
    };
    let mut found: Vec<Message> = state.store.search_messages(&search).await?;
    if q.has.iter().any(|h| h == "link") {
        found.retain(|m| m.text.contains("http://") || m.text.contains("https://"));
    }
    if q.is.iter().any(|i| i == "thread") {
        found.retain(|m| m.thread_ts.is_some());
    }
    if q.has.iter().any(|h| h == "pin") {
        let mut pinned = Vec::new();
        for m in found {
            if state
                .store
                .pinned(&m.channel_id, std::slice::from_ref(&m.ts))
                .await?
                .is_empty()
            {
                continue;
            }
            pinned.push(m);
        }
        found = pinned;
    }
    if !newest_first(a) {
        found.reverse();
    }
    let total = found.len();
    let (count, page, pagination, paging) = paging(a, total);
    let mut matches = Vec::new();
    for m in found.iter().skip((page - 1) * count).take(count) {
        let Some(conv) = state.store.get_conversation(&m.channel_id).await? else {
            continue;
        };
        let mut v = shapes::one_message(state, m).await?;
        let username = match &m.user_id {
            Some(id) => state.store.get_user(id).await?.map(|u| u.name).unwrap_or_default(),
            None => String::new(),
        };
        v["iid"] = json!(format!("{}-{}", m.channel_id, m.ts));
        v["score"] = json!(0);
        v["channel"] = match_channel(&conv, &me.id);
        v["username"] = json!(username);
        v["permalink"] = json!(super::chat::permalink(state, &conv, m));
        v["no_reactions"] = json!(v.get("reactions").is_none());
        matches.push(v);
    }
    Ok(json!({"total": total, "pagination": pagination, "paging": paging, "matches": matches}))
}

async fn file_results(state: &AppState, caller: &Caller, a: &Args) -> ApiResult<Value> {
    let q = parse(&req_str(a, "query")?);
    let channel = match &q.in_ {
        Some(raw) => resolve_in(state, caller, raw)
            .await?
            .map(|c| c.id)
            .or(Some(String::new())),
        None => None,
    };
    let user = match &q.from {
        Some(raw) => Some(resolve_user(state, caller, raw).await?.unwrap_or_default()),
        None => None,
    };
    let (after, before) = ts_bounds(&q);
    let terms: Vec<String> = q.terms.iter().map(|t| t.to_lowercase()).collect();
    let mut found: Vec<_> = visible_files(state, caller)
        .await?
        .into_iter()
        .filter(|(f, shares)| {
            let hay = format!("{} {}", f.name, f.title).to_lowercase();
            let created = ts_of(f.created);
            terms.iter().all(|t| hay.contains(t.as_str()))
                && user.as_ref().is_none_or(|u| *u == f.user_id)
                && channel
                    .as_ref()
                    .is_none_or(|c| shares.iter().any(|s| s.channel_id == *c))
                && after.as_ref().is_none_or(|a| created >= *a)
                && before.as_ref().is_none_or(|b| created < *b)
        })
        .collect();
    if !newest_first(a) {
        found.reverse();
    }
    let total = found.len();
    let (count, page, pagination, paging) = paging(a, total);
    let mut matches = Vec::new();
    for (f, shares) in found.iter().skip((page - 1) * count).take(count) {
        matches.push(file_object(state, f, shares).await?);
    }
    Ok(json!({"total": total, "pagination": pagination, "paging": paging, "matches": matches}))
}

pub async fn messages(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let results = message_results(state, caller, a).await?;
    Ok(json!({"ok": true, "query": req_str(a, "query")?, "messages": results}))
}

pub async fn files(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let results = file_results(state, caller, a).await?;
    Ok(json!({"ok": true, "query": req_str(a, "query")?, "files": results}))
}

pub async fn all(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let messages = message_results(state, caller, a).await?;
    let files = file_results(state, caller, a).await?;
    Ok(json!({
        "ok": true,
        "query": req_str(a, "query")?,
        "messages": messages,
        "files": files,
        "posts": {"total": 0, "matches": []},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_phrases() {
        let q = parse(r#"deploy "rolled back" in:#Ops from:@Alice after:2026-10-01 has:link is:thread"#);
        assert_eq!(q.terms, ["deploy", "rolled back"]);
        assert_eq!(q.in_.as_deref(), Some("#Ops"));
        assert_eq!(q.from.as_deref(), Some("@Alice"));
        assert_eq!(q.after.as_deref(), Some("2026-10-01"));
        assert_eq!(
            (q.has.as_slice(), q.is.as_slice()),
            (&["link".to_owned()][..], &["thread".to_owned()][..])
        );
    }

    #[test]
    fn day_bounds() {
        assert_eq!(day_start("2026-10-29"), Some(1_793_232_000));
        assert_eq!(day_start("2026-13-01"), None);
        let q = parse("on:2026-10-29");
        let (after, before) = ts_bounds(&q);
        assert_eq!(after.as_deref(), Some("1793232000.000000"));
        assert_eq!(before.as_deref(), Some("1793318400.000000"));
        // `after:` excludes the day itself.
        assert_eq!(
            ts_bounds(&parse("after:2026-10-29")).0.as_deref(),
            Some("1793318400.000000")
        );
    }
}
