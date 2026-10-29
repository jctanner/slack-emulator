# slack-emu — Design

A self-hosted Slack emulator for developing and testing Slack bots/apps. Real bot
code (Bolt for Python/JS, `slack_sdk`, etc.) should run against it unmodified
except for the API base URL. Humans use it through a Slack-like web UI on
desktop and mobile browsers.

## Goals

1. **Wire compatibility for bots** — Web API methods, payload shapes, error
   codes, token types, Events API (HTTP) and Socket Mode (WebSocket),
   interactivity (Block Kit actions, modals, slash commands).
2. **A usable web chat UI** — channels (public/private), DMs, threads,
   reactions, mentions, real-time updates, responsive layout.
3. **Test-harness ergonomics** — seed from fixtures, reset state, inspect every
   API call and event delivery, deterministic behaviour where possible.
4. **Trivial to run** — one container, SQLite by default, Postgres optional.

## Non-goals (for now)

Huddles/calls, Slack Connect, Enterprise Grid, Workflow Builder, canvases,
lists, full-text search beyond simple `LIKE`, native desktop/mobile clients,
SAML/SCIM. File uploads: basic support later.

## Architecture

```
                 ┌──────────────────────── slack-emu (single binary) ─────────────────────────┐
 Browser ──HTTP──┤ /            static SPA (React, embedded via rust-embed)                    │
   (UI)  ──WS────┤ /ws/client   UI real-time stream (session-cookie auth)                     │
                 │                                                                            │
 Bot ─────HTTP───┤ /api/{method}   Slack Web API  ──┐                                         │
 (Bolt)  ──WS────┤ /ws/socket-mode Socket Mode      │    ┌──────────┐     ┌───────────────┐   │
                 │                                  ├──▶ │ services │ ──▶ │ store (sqlx)  │   │
 Tests ───HTTP───┤ /_emu/*        admin/test API  ──┘    └────┬─────┘     │ sqlite | pg   │   │
                 │                                            │ events    └───────────────┘   │
                 │                                      ┌─────▼──────┐                        │
                 │                                      │ event bus  │──▶ UI WS fan-out       │
                 │                                      │ (tokio)    │──▶ Socket Mode conns   │
                 │                                      └────────────┘──▶ HTTP dispatcher ────┼──▶ bot request_url
                 └────────────────────────────────────────────────────────────────────────────┘
```

**The web UI calls the same `/api/*` methods as bots**, authenticated by
session cookie instead of a bearer token (like Slack's own web client). One
code path handles all writes, so the UI exercises the same behaviour bots see.

### Stack

| Concern        | Choice                                                        |
|----------------|---------------------------------------------------------------|
| HTTP / WS      | `axum` + `tokio` (+ `tower-http` for tracing, CORS, compression) |
| Database       | `sqlx` — SQLite default, Postgres via `DATABASE_URL`          |
| Migrations     | `sqlx migrate`, separate dirs `migrations/sqlite`, `migrations/postgres` |
| Password hash  | `argon2`                                                      |
| Outbound HTTP  | `reqwest` (Events API delivery)                               |
| Serialization  | `serde` / `serde_json`                                        |
| Frontend       | React + TypeScript + Vite, built into `web/dist`, embedded in binary |
| Container      | multi-stage build → minimal image, `/data` volume for SQLite  |

### Database portability

Access goes through a `Store` trait (repository pattern) and nothing else
talks to the database directly. Both backends run the same statements: the
SQL is written once (`crates/store/src/backend.rs`) and a macro builds each
backend from it. Each backend supplies its row types and a `prep()` that
adapts a statement to its dialect; Postgres numbers the `?` placeholders.
The SQL keeps to the common subset: `TRUE`/`FALSE`, `LIKE`, `ON CONFLICT`,
and JSON stored as TEXT and handled in Rust. The Postgres schema adds
`COLLATE "C"` on IDs and timestamps, so they sort byte-wise like SQLite
text, and an explicit `rowid` where SQLite's implicit one orders rows.

Migrations live in `migrations/sqlite` and `migrations/postgres`, numbered in
step. A conformance suite in the store crate, and the whole server
integration suite, run on both backends (`scripts/test-matrix.sh`).

## Domain model

Slack-style IDs, generated with the correct prefixes so bot code that pattern
matches on them keeps working:

| Entity        | Prefix | Notes |
|---------------|--------|-------|
| Workspace     | `T`    | Multiple workspaces, switchable in the UI rail |
| Account       | —      | Global login identity (emulator-internal, never exposed to bots) |
| User          | `U`    | Per-workspace profile; humans *and* bot users |
| Channel       | `C`    | Public and private (Slack now uses `C` for both; `is_private` flag) |
| DM / MPIM     | `D` / `C` (`is_mpim`) | |
| App           | `A`    | Owns bot user, tokens, event subscriptions, URLs |
| Bot           | `B`    | `bot_id` on bot messages |
| Message       | `ts`   | `"1730212345.000100"`, unique per channel, monotonic |
| File (later)  | `F`    | |

Core tables: `workspaces`, `accounts`, `users`, `identities`, `sessions`, `conversations`,
`conversation_members`, `messages` (with `thread_ts`, `blocks` JSON, `edited`,
`subtype`), `reactions`, `apps`, `app_tokens`, `app_installations`,
`event_subscriptions`, `slash_commands`, `views` (modals), `event_deliveries`,
`api_log`.

### Auth design (leave room for OAuth/OIDC)

- **`accounts`** is the global person who logs in (email, display name).
  One account can be a member of many workspaces.
- **`users`** is the Slack-facing, per-workspace profile (`U…` id, name,
  real_name, avatar, tz, is_bot, role). As in real Slack, one person has a
  different user ID in each workspace they belong to. Bot users have no
  account.
- **`identities`** maps *how you log in* to an account:
  `(account_id, provider, subject, secret_hash)`. Phase 1 has only
  `provider = "password"`. Adding OIDC later means adding
  `provider = "oidc:<issuer>"` rows. Neither the account model nor the
  session model changes.
- **`sessions`**: opaque random token in an HttpOnly cookie, stored hashed,
  bound to an account. The UI tells `/api/*` which workspace it is acting in
  (`X-Slack-Emu-Team` header, defaulting to the account's only workspace).
  This plays the role of Slack's per-workspace `xoxc` client tokens.
- **API tokens** (`app_tokens`): `xoxb-` (bot), `xoxp-` (user, issued via app
  install), `xapp-` (app-level, Socket Mode). Each has scopes, which are
  checked per method so `missing_scope` errors work as they do in Slack.
- **App install**: phase 1 provisions tokens from the admin UI/fixtures. The
  schema and `oauth.v2.access` shape are designed so a real OAuth install
  flow (`/oauth/v2/authorize` → code → `oauth.v2.access`) can be added
  without migrating tokens.

## Bot-facing surface

### Web API (`POST|GET /api/{method}`)

Accepts `application/json` *and* `application/x-www-form-urlencoded`, and
takes the token from the `Authorization: Bearer` header or a `token` param, as
Slack does. Responses always use the `{"ok": bool, "error"?: str, ...}`
envelope. Errors use Slack's error strings (`channel_not_found`,
`not_in_channel`, `invalid_auth`, `missing_scope`, `ratelimited`, ...). List
methods support cursor pagination via `response_metadata.next_cursor`.

Initial method set (roughly what Bolt apps use day to day):

- `api.test`, `auth.test`
- `chat.postMessage`, `chat.update`, `chat.delete`, `chat.postEphemeral`,
  `chat.getPermalink`, `chat.meMessage`
- `conversations.list|info|history|replies|members|create|join|leave|invite|kick|archive|unarchive|rename|setTopic|setPurpose|open|mark`
- `users.list|info|lookupByEmail|conversations`, `users.profile.get`
- `reactions.add|remove|get`
- `views.open|update|push|publish` (modals + App Home)
- `bots.info`, `team.info`
- `apps.connections.open` (Socket Mode)

Later: `files.*` (v2 upload flow), `pins.*`, `bookmarks.*`, `search.messages`,
`chat.scheduleMessage`, `usergroups.*`.

### Events

When something happens in a service, it emits a typed domain event onto an
in-process bus. Three subscribers consume it:

1. **UI fan-out**: pushes to browser sessions that are members of the
   conversation.
2. **Socket Mode**: for each app with active Socket Mode connections and a
   matching subscription, sends an envelope
   (`{envelope_id, type: "events_api", payload, accepts_response_payload}`)
   and expects an `ack` from the bot. Includes the `hello` handshake and
   `disconnect` messages.
3. **HTTP dispatcher**: POSTs the `event_callback` JSON to the app's
   request URL, with `X-Slack-Signature` / `X-Slack-Request-Timestamp`
   (HMAC-SHA256 using the app's signing secret). Supports the
   `url_verification` challenge and retries with `X-Slack-Retry-Num` /
   `X-Slack-Retry-Reason`.

Event types for phase 1: `message` (+ subtypes `message_changed`,
`message_deleted`, `bot_message`, `thread_broadcast`), `app_mention`,
`reaction_added/removed`, `member_joined_channel`, `member_left_channel`,
`channel_created/rename/archive`, `app_home_opened`, `team_join`.

Delivery honours Slack's visibility rules: a bot only gets `message.channels`
events for channels it's a member of, `message.im` for its DMs, etc., based on
subscribed event types and token scopes.

### Interactivity

- **Block Kit**: messages store `blocks`, the UI renders them, and the
  emulator validates the common block and element types.
- **Actions**: clicking a button or select in the UI sends a `block_actions`
  payload with a `trigger_id` and `response_url` over Socket Mode or HTTP
  (form-encoded `payload=`, signed).
- **Modals**: `views.open/push/update` render in the UI. Submitting sends
  `view_submission`, and the bot's `response_action` (`errors`, `update`,
  `push`, `clear`) is honoured. Closing sends `view_closed`.
- **Slash commands**: registered per app. Typing `/cmd` in the composer sends
  the command payload. Also supports `response_url` and in-channel versus
  ephemeral responses.
- **Shortcuts** (global and message): later phase.

### `mrkdwn`

Shared formatting rules, with a renderer in the frontend:
`*bold*`, `_italic_`, `~strike~`, `` `code` ``, code blocks, quotes, lists,
`<@U123>`, `<#C123>`, `<!here>`, `<!channel>`, `<url|label>`. The server
parses mentions to work out `app_mention` delivery and notifications.

## Test-harness API (`/_emu/*`)

Disabled unless `EMU_ADMIN_TOKEN` is set. Not part of Slack's API. The
reference is [HARNESS.md](HARNESS.md).

- `POST /_emu/reset`: wipe all state and recreate the default workspace.
- `POST /_emu/seed`: load a YAML/JSON fixture (workspaces, people with
  passwords, apps with fixed tokens and secrets, channels, members, messages,
  replies and reactions). Seeding is additive and replays Web API calls, so
  seeded data matches data made by hand.
- `GET /_emu/api-log`, `/_emu/deliveries`, `/_emu/events`: ring buffers
  with sequence numbers for polling.
- `POST /_emu/as/{user}/{method}`: act as a person with the web UI's
  permissions.
- `GET /_emu/wait`: long-poll for a matching event after a seq.
- `/_emu/rate-limits`: Slack-like tiers per app per workspace, plus forced
  429s for testing retries.

The web UI's **Inspector** (owners and admins) shows the same logs for one
workspace.

## Web UI

- Layout follows Slack's: workspace rail (switch, create, join workspaces) → sidebar (channels, DMs, apps) →
  message pane → optional right pane (thread, profile, inspector).
- Mobile (< 768px): one pane at a time. The sidebar becomes the home screen,
  tapping a conversation pushes the message view, and threads push on top.
- Features: login, channel browse/create/join/leave, private channels and
  invites, DMs and group DMs, threads ("also send to channel"), reactions,
  edit/delete, mentions with autocomplete, unread state, typing indicators,
  Block Kit rendering, modals, App Home tab, slash-command autocomplete.
- The look is Slack-*like*, not a pixel-perfect copy (no Slack branding or
  assets).

## Configuration

Environment variables (CLI flags mirror them):

| Var | Default | |
|-----|---------|--|
| `BIND_ADDR` | `0.0.0.0:8080` | |
| `PUBLIC_URL` | `http://localhost:8080` | used in permalinks, `response_url`, Socket Mode `wss://` URL |
| `DATABASE_URL` | `sqlite:///data/slack-emu.db` | or `postgres://...` |
| `EMU_ADMIN_TOKEN` | unset | enables `/_emu/*` |
| `EMU_SEED_FILE` | unset | fixture loaded at startup when the database has no users |
| `EMU_ALLOW_SIGNUP` | `true` | open registration vs admin-created users |

## Pointing bots at the emulator

```python
# Bolt for Python
from slack_sdk import WebClient
from slack_bolt import App
client = WebClient(token="xoxb-...", base_url="http://localhost:8080/api/")
app = App(client=client, signing_secret="...")
```

```js
// Bolt for JS
const app = new App({
  token: 'xoxb-...', appToken: 'xapp-...', socketMode: true,
  clientOptions: { slackApiUrl: 'http://localhost:8080/api/' },
});
```

Socket Mode needs no extra config. `apps.connections.open` returns a
`ws://<PUBLIC_URL>/ws/socket-mode?...` URL and the SDK connects to it.

## Repository layout

```
Cargo.toml              workspace
crates/
  server/               axum app, routes, WS, config, main
  core/                 domain types, IDs, services, events, mrkdwn parsing
  store/                Store trait + sqlite/postgres impls, migrations
web/                    React + Vite SPA
compat/
  python/               Bolt-Python & slack_sdk test bots + pytest suite
  js/                   Bolt-JS test bots + test suite
fixtures/               example seed files
Containerfile
docs/
```

## Milestones

1. **Skeleton**: cargo workspace, config, SQLite store and migrations,
   password auth and sessions, embedded SPA shell, `/api/auth.test`,
   Containerfile.
2. **Chat core**: workspaces, users, channels (public and private), DMs,
   messages, threads, reactions. UI with real-time WS, responsive layout.
3. **Bots, part 1**: apps, bot users, tokens and scopes, the Web API
   methods above, Socket Mode, Events API with signing and retries.
   Python and JS Bolt compat suites run in CI.
4. **Interactivity**: Block Kit rendering, `block_actions`, modals,
   slash commands, App Home, ephemeral messages, `response_url`.
5. **Harness**: `/_emu/*` (seed, reset, logs, act-as, wait), Inspector
   UI, rate-limit emulation.
6. **Postgres**: second Store impl, CI matrix.
7. **Later**: files, OAuth app install flow, OIDC login, search, pins,
   bookmarks, scheduled messages, shortcuts, user groups, message retention
   policies (workspace default plus per-channel override, mirroring Slack's
   admin setting and `admin.conversations.*CustomRetention`).

## Decisions log

- Multiple workspaces in the UI from the start (2026-10-29).
- Bot messages show an "APP" badge and app profile card (2026-10-29).
- History is kept forever by default and cleared with `/_emu/reset`.
  Optional retention policies are a later milestone (2026-10-29).
- Milestone 2 permission choices (2026-10-29), all permissive defaults
  that Slack admins can also choose:
  - Any channel member can rename, archive, set the topic or purpose, and
    remove members.
  - People can read public channels without joining. Bots must join first
    (`not_in_channel`), until scopes such as `chat:write.public` arrive in
    milestone 3.
  - Only authors can edit their messages. Authors and workspace
    admins/owners can delete them.
- Deleting a thread parent that has replies leaves a `tombstone`, which is
  removed together with the last reply.
- Every workspace gets `#general` (everyone joins and nobody can leave) and
  `#random`.
- Milestone 3 app decisions (2026-10-29):
  - Apps are defined only by Slack app manifests (`apps.manifest.*`).
    Installing is a button in the UI (`emu.apps.install`), which returns the
    `oauth.v2.access` response shape. A real OAuth install flow can be added
    later without changing tokens.
  - `apps.manifest.*` and `emu.*` accept only the web UI session for now. The
    harness admin token (milestone 5) will open them to scripts.
  - Scope changes need a reinstall, as in Slack. Reinstalling keeps the
    same bot token.
  - Socket Mode takes precedence over a request URL. HTTP events go only to a
    request URL that passed `url_verification`.
  - Events are retried after a failure, an HTTP error, or a missing ack
    within 3 s, using Slack's schedule (configurable). Bolt for Python
    deliberately doesn't ack events no listener handles, so those are retried
    too, as on Slack.
  - Installing or uninstalling announces the bot user with `user_change`.
- Milestone 4 interactivity decisions (2026-10-29):
  - Trigger IDs, response URLs, modal stacks and ephemeral messages live only
    in memory, like Slack's short-lived handles. A restart closes open modals
    and expires response URLs. App Home views are stored (`home_views`).
  - Trigger IDs expire after 3 s (`EMU_TRIGGER_TTL`) and work once. Response
    URLs allow 5 posts in 30 minutes.
  - Buttons, menus and modals need `settings.interactivity.is_enabled`, as on
    Slack. Slash commands need the `commands` scope. HTTP apps need a URL for
    each. These are checked at manifest validation.
  - Interactions and slash commands are not retried. A failure shows in the
    UI: a warning by the element, the modal's "trouble connecting" message, or
    Slackbot's `failed with the error "dispatch_failed"` / `operation_timeout`.
  - An `in_channel` immediate slash command reply also posts the command
    itself, as Slack does. Response URL replies can post anywhere the
    interaction happened, without the bot being a member.
  - Apps updating their own messages (`chat.update` by a bot,
    `replace_original`) don't get the "(edited)" mark. `replace_original`
    replaces the whole message, so blocks not resent are dropped.
  - Required `input` blocks are checked before a submission reaches the app,
    in the UI and again on the server (`emu.views.submit`).
  - A modal's Cancel/Back pops one view and × closes the stack.
    `view_closed` is sent for the view the user closed, with `is_cleared`
    when the whole stack went.
  - The UI calls Slack's internal `chat.command` for slash commands, plus
    `emu.*` session methods for actions, submissions, modal state and App
    Home. Scripts can use them to act as a person.
- Milestone 5 harness decisions (2026-10-29):
  - One admin token (`EMU_ADMIN_TOKEN`) guards all of `/_emu`. Without it,
    the harness returns 404 `harness_disabled`.
  - "Act as" uses the web UI's permissions, so session-only methods
    (`chat.command`, `emu.*`) work too. Bots act through their own tokens.
  - Logs are in memory (2000 API calls, 200 deliveries per app, 5000 events)
    and cleared by reset. Event sequence numbers keep increasing across
    resets, so an old `after` never matches new events by accident.
  - Events are stored on the bus with a seq, so `/_emu/wait` sees events that
    happened before the wait started. That avoids races between "act" and
    "wait".
  - Rate limits are off by default and apply only to API tokens. Forced
    limits work even when limits are off. Tiers follow Slack's docs
    approximately; `chat.postMessage` is limited per channel.
  - Fixtures need a person to create channels and install apps. They use the
    first owner/admin in the fixture unless one is named.
  - Fixture YAML is parsed with `serde-saphyr` (`serde_yaml` is
    unmaintained).
- Milestone 6 Postgres decisions (2026-10-29):
  - The SQL is shared, not duplicated per backend, so the backends can't
    drift. Dialect differences live in `prep()` and the schema.
  - Tests pick Postgres with `EMU_TEST_POSTGRES` (a server URL). Each test
    gets its own `emu_test_*` database, which is left behind, so use a
    throwaway server; `scripts/test-matrix.sh` starts one in a container.
  - No CI configuration is checked in yet (the hosting isn't decided).
    `scripts/test-matrix.sh` is the one command a CI job needs.
  - The harness's API log is written when a call returns, so it can trail
    the events the call caused by a moment. Waiting on events is the
    reliable signal.
- `client.counts` (an undocumented method from Slack's own web client) drives
  the unread state in the sidebar. Events Slack doesn't have are prefixed
  `emu_` and are never delivered to apps.
