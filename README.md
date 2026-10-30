# slack-emu

A self-hosted Slack emulator for developing and testing Slack bots. Real bot
code (Bolt, `slack_sdk`, …) talks to it by changing only the API base URL. People
use it through a Slack-like web UI in any desktop or mobile browser.

See [docs/DESIGN.md](docs/DESIGN.md) for the architecture and roadmap.

**Status: milestone 7 (files, external selects, shortcuts, pins, bookmarks, scheduled messages, user groups, search and message retention).** Accounts and multiple workspaces;
public and private channels, DMs and group DMs, threads, reactions, unread
tracking and live updates in a responsive web UI. Apps are created from Slack
app manifests and installed for bot tokens with enforced scopes. Events are
delivered over Socket Mode or signed HTTP with Slack's retries. Messages,
modals and App Home tabs render Block Kit; buttons, menus, modals and slash
commands reach apps as `block_actions`, `view_submission`, `view_closed` and
slash command payloads, with trigger IDs and response URLs. Real Bolt for
Python and Bolt for JavaScript apps run against it unchanged (see
`compat/`). A test harness (`/_emu/*`) resets and seeds the emulator from
fixtures with fixed tokens, acts as people, waits for events, logs every API
call and delivery, and emulates Slack's rate limits. Workspace admins get a
live Inspector in the UI.

Web API methods:

- `api.test`, `auth.test`, `auth.revoke`, `team.info`, `bots.info`
- `admin.conversations.setCustomRetention|getCustomRetention|removeCustomRetention`
- `bookmarks.add|edit|list|remove`, `pins.add|list|remove`
- `apps.connections.open`, `apps.manifest.create|update|delete|export|validate`
- `chat.postMessage`, `chat.postEphemeral`, `chat.update`, `chat.delete`, `chat.meMessage`, `chat.getPermalink`
- `chat.scheduleMessage`, `chat.scheduledMessages.list`, `chat.deleteScheduledMessage`
- `conversations.archive|close|create|history|info|invite|join|kick|leave|list|mark|members|open|rename|replies|setPurpose|setTopic|unarchive`
- `files.getUploadURLExternal|completeUploadExternal|info|list|delete` (what `files_upload_v2` / `files.uploadV2` use; the retired `files.upload` answers `method_deprecated`, as on Slack)
- `reactions.add|get|remove`
- `search.messages|files|all` (people and user tokens; bots get `not_allowed_token_type`, as on Slack)
- `users.conversations|info|list|lookupByEmail|profile.get`
- `usergroups.create|disable|enable|list|update`, `usergroups.users.list|update`
- `views.open|push|update|publish`

Events API: `message.channels|groups|im|mpim`, `app_mention`,
`reaction_added|removed`, `member_joined_channel|member_left_channel`,
`channel_*`/`group_*` lifecycle events, `im_created`, `team_join`,
`user_change`, `app_home_opened`, `app_uninstalled`, `file_shared`,
`file_deleted`, `pin_added`, `pin_removed` and the `subteam_*` user group
events.

Interactivity: `block_actions` (messages, ephemeral messages, modals, App
Home), `view_submission` (with `response_action` `errors`, `update`, `push`
and `clear`), `view_closed`, and slash commands. Response URLs support
`in_channel`/ephemeral replies, `replace_original`, `delete_original` and
`thread_ts`. Blocks are validated like Slack does (`invalid_blocks` with
`[ERROR] … [json-pointer:…]` messages) and missing `block_id`/`action_id`s are
filled in.

External selects load their options from the app as people type
(`block_suggestion`, answered with `app.options` in Bolt), and global and
message shortcuts (`shortcut`, `message_action`) run from the composer's ⚡
menu and a message's "⋯" menu. File inputs and workflow buttons render but
don't work yet.

Storage is SQLite by default or Postgres, with the same behaviour (the
whole test suite runs on both).

Files can be uploaded by bots and people (attach, paste or drag into the
composer), and are shown in messages: images inline, other files as download
cards. `url_private` downloads need a token with `files:read` or a signed-in
session, as on Slack (the emulator answers 401 rather than Slack's login
page).

Messages can be pinned (📌 in a message's actions; "Pinned" in channel
details), and channels have a bookmarks bar.

Messages can be scheduled, by bots or from the composer (🕒), and are
posted when due, even across restarts.

User groups are managed from the account menu, suggested in the composer,
and mentioning one (`<!subteam^S…|@handle>`) notifies its members.

Search (the box at the top of the sidebar) covers messages and files, with
Slack's `in:`, `from:`, `before:`/`after:`/`on:`, `has:link`, `has:pin`,
`is:thread` and quoted phrases.

Workspace admins can set how long messages are kept (account menu →
Workspace settings), and override it per channel (channel settings, or
`admin.conversations.setCustomRetention`). By default messages are kept
forever.

Later: an OAuth app install flow and OIDC sign-in (see
[the roadmap](docs/DESIGN.md#milestones)).

## Run it

```sh
podman build -t slack-emu -f Containerfile .
podman run --rm -p 8080:8080 -v slack-emu-data:/data slack-emu
```

Open http://localhost:8080 and create an account. The first account becomes
owner of the default workspace.

To use Postgres instead of the SQLite file in `/data`, point `DATABASE_URL`
at an empty database. The emulator creates its tables on start:

```sh
podman run --rm -p 8080:8080 \
  -e DATABASE_URL=postgres://slack:secret@db.example:5432/slack_emu slack-emu
```

It's tested with Postgres 16. To start with data, mount a fixture (see
[docs/HARNESS.md](docs/HARNESS.md#fixtures)) and set `EMU_SEED_FILE`. It
loads when the database has no users yet:

```sh
podman run --rm -p 8080:8080 -v ./fixture.yaml:/fixture.yaml:ro,Z \
  -e EMU_SEED_FILE=/fixture.yaml slack-emu
```

## Develop

```sh
# backend (http://localhost:8080)
cargo run --bin slack-emu

# frontend with hot reload (http://localhost:5173, proxies to the backend)
cd web && npm install && npm run dev

# tests
cargo test                         # Rust tests on SQLite
scripts/test-matrix.sh             # everything: Rust and Bolt suites on SQLite and Postgres
```

`scripts/test-matrix.sh` starts a throwaway Postgres container (podman or
docker), or uses the server in `EMU_TEST_POSTGRES` if set. It's also meant
as the single step of a CI job. `EMU_TEST_POSTGRES=postgres://… cargo test`
runs just the Rust tests on Postgres, with a fresh database per test.

A release build embeds `web/dist` into the binary, so run `npm run build` in
`web/` first. Debug builds read `web/dist` from disk.

## Configuration

| Variable | Default | |
|---|---|---|
| `BIND_ADDR` | `0.0.0.0:8080` | |
| `PUBLIC_URL` | `http://localhost:8080` | externally reachable URL; `https://` turns on secure cookies |
| `DATABASE_URL` | `sqlite://slack-emu.db` (`sqlite:///data/slack-emu.db` in the container) | or `postgres://user:pass@host/db` |
| `EMU_DEFAULT_WORKSPACE_NAME` | `Emulator` | created on first start; new sign-ups join it |
| `EMU_DEFAULT_WORKSPACE_DOMAIN` | `emulator` | |
| `EMU_ALLOW_SIGNUP` | `true` | |
| `EMU_SESSION_TTL_SECS` | `2592000` (30 days) | |
| `EMU_EVENT_RETRY_DELAYS` | `1,60,300` | seconds before each Events API retry; empty disables retries |
| `EMU_ADMIN_TOKEN` | unset | turns on the test harness at `/_emu/*`; send it as a bearer token |
| `EMU_MAX_FILE_BYTES` | `52428800` (50 MiB) | largest file upload; files are stored in the database |
| `EMU_SEED_FILE` | unset | fixture (YAML or JSON) to load at startup when the database has no users |
| `EMU_RATE_LIMITS` | `false` | start with Slack-like rate limits on (toggle at runtime with `/_emu/rate-limits`) |
| `EMU_TRIGGER_TTL` | `3` | seconds a `trigger_id` stays usable; raise it while stepping through a bot in a debugger |
| `RUST_LOG` | `slack_emu=info,tower_http=info` | |

## Building a bot against it

1. In the web UI, open **Apps → Manage apps → Create an app** and paste a
   Slack app manifest (YAML or JSON). The template enables Socket Mode,
   interactivity, a `/hello` command and a Home tab.
2. Click **Install** to get a bot token (`xoxb-…`). For Socket Mode, also
   generate an app-level token (`xapp-…`, `connections:write`).
3. Point your SDK at the emulator's Web API:

```python
# Bolt for Python, Socket Mode
from slack_bolt import App
from slack_bolt.adapter.socket_mode import SocketModeHandler
from slack_sdk import WebClient

api = "http://localhost:8080/api/"
app = App(client=WebClient(token="xoxb-…", base_url=api))
SocketModeHandler(app, "xapp-…", web_client=WebClient(base_url=api)).start()
```

```js
// Bolt for JavaScript, Socket Mode
const app = new App({
  token: 'xoxb-…',
  appToken: 'xapp-…',
  socketMode: true,
  clientOptions: { slackApiUrl: 'http://localhost:8080/api/' },
})
```

For HTTP delivery instead, set `settings.event_subscriptions.request_url`
(and `settings.interactivity.request_url` and each slash command's `url`) in
the manifest and verify requests with the app's signing secret. As with
Slack, events are sent only after the URL answers the `url_verification`
challenge, and the app page has a **Retry verification** button. Each app's
**Event log** tab shows every delivery attempt with its payload, so you can
see what your bot received or missed.

Invite the bot to a channel (channel details → Members → Add people) to
receive its messages. A scope missing from the manifest returns
`missing_scope` with `needed`/`provided`, just like Slack.

Type `/` in the composer to see slash commands, and use **Open app** on the
app page (or the app's DM in the sidebar) for its Home, Messages and About
tabs. If an app doesn't acknowledge an interaction within 3 seconds, the UI
says so next to the button or in the modal, and Slackbot reports failed
slash commands, as on Slack. Response URL posts appear in the event log as
`response_url`.

Scripts and tests can act as a person through the same session-authenticated
methods the web UI uses: `chat.command`, `emu.blocks.action`,
`emu.views.submit`, `emu.views.close`, `emu.views.current`, `emu.blocks.suggest`,
`emu.shortcuts.list`, `emu.shortcuts.run` and `emu.apps.home` (see `compat/`
for examples).

## Testing bots in CI

Set `EMU_ADMIN_TOKEN` and use the test harness: reset, load a fixture with
fixed tokens (`xoxb-test-…`), start your bot, act as people, and wait for the
bot's reply:

```sh
H="Authorization: Bearer $EMU_ADMIN_TOKEN"
curl -H "$H" -X POST localhost:8080/_emu/reset
curl -H "$H" -H 'Content-Type: application/yaml' --data-binary @fixture.yaml localhost:8080/_emu/seed
curl -H "$H" -D - -d '{"channel": "C…", "text": "status?"}' localhost:8080/_emu/as/alice/chat.postMessage   # X-Emu-Seq: 41
curl -H "$H" 'localhost:8080/_emu/wait?after=41&type=message&channel=C…&contains=green'
```

See [docs/HARNESS.md](docs/HARNESS.md) for the endpoints, the fixture
format and rate limits.

## Compatibility tests

`compat/run.sh` starts a throwaway emulator and runs real Bolt apps against
it. It covers Bolt for Python and Bolt for JavaScript, each over Socket Mode
and HTTP: messages, mentions, slash commands with `respond()`, a button that
opens a modal, a submission rejected with `response_action: errors` and then
accepted, and an App Home published on `app_home_opened`. A harness test
seeds a fixture, drives a Bolt bot with fixed tokens through `/_emu/as` and
`/_emu/wait`, and checks that `slack_sdk` retries through a forced rate limit.
It needs `python3` and `npm`.
