# Test harness

The harness lets a test suite set up the emulator, play the people in it,
and check what a bot did, without driving a browser. It lives under
`/_emu/` and isn't part of Slack's API.

It's off unless the server has `EMU_ADMIN_TOKEN` set, and every request
must send that token:

```sh
export EMU_ADMIN_TOKEN=dev-secret       # on the server
curl -H "Authorization: Bearer $EMU_ADMIN_TOKEN" http://localhost:8080/_emu/seq
```

Errors look like Slack's (`{"ok": false, "error": "...", "message": "..."}`)
with an HTTP status that says what went wrong.

## A typical test

```python
h.reset()                                         # POST /_emu/reset
ids = h.seed(FIXTURE)                             # POST /_emu/seed
start_my_bot(token="xoxb-test-build")             # fixed token from the fixture
_, seq = h.act("alice", "chat.postMessage", channel=ids["channels"]["builds"], text="status?")
reply = h.wait(after=seq, type="message", user=bot_user, contains="green")   # GET /_emu/wait
```

`compat/python/emu.py` has a small `Harness` client, and
`compat/python/test_harness.py` is a complete example with Bolt.

## Endpoints

| Endpoint | |
|---|---|
| `POST /_emu/reset` | Deletes everything: workspaces, people, apps, messages, sessions, logs. Socket Mode connections are closed. The default workspace is recreated. |
| `POST /_emu/seed` | Loads a fixture (YAML or JSON, see below). Returns the IDs, tokens and secrets it created. |
| `POST /_emu/as/{user}/{method}` | Calls a Web API method as a person, with the web UI's permissions. `{user}` is a user ID, handle or email; add `?team=<id or domain>` if it's ambiguous. Arguments go in the body (JSON or form) or query. The `X-Emu-Seq` response header is the event seq just before the call. |
| `GET /_emu/wait` | Waits for the first event after `after` that matches the filters, for up to `timeout` seconds (default 10, max 120). Returns `{event: {seq, team_id, audience, ui_only, event}}`, or 408 with `"error": "timeout"`. |
| `GET /_emu/events` | Recent events (the last 5000) with the same filters. |
| `GET /_emu/seq` | The latest event seq. |
| `GET /_emu/api-log` | Recent Web API calls (the last 2000): method, caller, arguments, response, status, latency. Tokens are shortened (`xoxb-…a1b2`). |
| `GET /_emu/deliveries` | Recent deliveries to apps: events, interactions, slash commands and their retries, plus `response_url` posts from apps. |
| `GET/POST /_emu/rate-limits` | Rate-limit status and settings. |

### Waiting for events

Events are what the web UI and apps hear about: `message` (including
`message_changed` and friends), `reaction_added`, `member_joined_channel`,
and so on. There are also the UI's own `emu_ephemeral` and `emu_modals`.
Filters:

- `type`, `subtype` (`subtype=none` for plain messages), `channel`, `user`,
  `text` (exact), `contains` (substring), `thread_ts`, `bot_id`, `ts`. Any
  other name matches that field of the event. It also matches the event's
  `message`, which covers edits and ephemeral messages.
- `team` (ID) and `to` (a user who received the event, e.g. for
  `emu_ephemeral` or `emu_modals`).
- `after`: only events with a larger seq. Take it from `X-Emu-Seq` (after
  `/_emu/as`), from a previous wait's `seq`, or from `/_emu/seq`. Without
  it, every remembered event counts, including ones from earlier steps.

### Logs

`api-log` filters: `method`, `app_id`, `team_id`, `user_id`, `via`
(`token`, `session` for the web UI, `harness`, `anonymous`), `errors=true`,
and `status` (or any other field, e.g. `status=429`). `deliveries` filters:
`app_id`, `label` (e.g. `message.channels`, `block_actions`, `/deploy`,
`response_url`), `errors=true`.

Both endpoints, and `events`, take `after=<seq>` for polling: that returns
the oldest entries after it, up to `limit`. Without `after` they return the
newest `limit` entries (default 200), oldest first.

A call is logged when it returns, so its entry can appear a moment after
the events it caused: `wait` for the event, then poll the log if you need
both.

Calls with a rejected token are logged too, with the shortened token, which
helps when a bot gets `invalid_auth`.

### Rate limits

Rate limits are off by default. Turn them on with `EMU_RATE_LIMITS=true` or
at runtime:

```sh
curl -H "Authorization: Bearer $EMU_ADMIN_TOKEN" -d '{"enabled": true}' http://localhost:8080/_emu/rate-limits
```

When they're on, API token calls are limited per app (or user) per
workspace. Each method gets roughly its tier from Slack's documentation:
1, 20, 50 or 100 calls a minute, allowing bursts. `chat.postMessage` is
limited per channel instead, to about 1 a second with bursts of 5. Over
the limit, a call gets HTTP 429, `{"ok": false, "error": "ratelimited"}`
and a `Retry-After` header. The web UI and `/_emu/as` aren't limited.

The `POST` body can also:

- `methods`: set a method's calls per minute, e.g. `{"methods": {"chat.update": 5}}`
  (`null` restores the default).
- `force`: fail a method's next N token calls whether or not limits are on,
  to test a bot's retry handling, e.g.
  `{"force": {"chat.postMessage": {"count": 2, "retry_after": 1}}}`.

`/_emu/reset` clears overrides and forced limits.

## Fixtures

Seeding adds to what's already there. Workspaces are matched by domain (use
`emulator` to add to the default workspace), people by handle and channels
by name. Everything except account creation goes through the same Web API
methods the UI uses, so seeded channels get the usual join messages and
events.

```yaml
workspaces:
  - domain: acme             # created if it doesn't exist
    name: Acme Inc
    users:
      - name: alice          # handle (@alice)
        real_name: Alice Anders
        email: alice@acme.test
        password: password123   # optional: lets alice sign in to the web UI
        owner: true          # or admin: true
      - name: bob            # no password: only reachable through /_emu/as
    apps:
      - manifest:            # a Slack app manifest, as for apps.manifest.create
          display_information: {name: Build Bot}
          features: {bot_user: {display_name: Build Bot}}
          oauth_config: {scopes: {bot: [chat:write, channels:history, channels:read]}}
          settings:
            socket_mode_enabled: true
            event_subscriptions: {bot_events: [message.channels]}
        bot_token: xoxb-test-build      # optional fixed tokens and secrets
        app_token: xapp-test-build      # Socket Mode apps get one anyway
        signing_secret: test-signing-secret
        # also: id, verification_token, client_id, client_secret,
        # installer (a handle; default: the first owner/admin), install: false
    channels:
      - name: builds
        private: false
        topic: CI results
        purpose: Where the build bot reports
        members: [alice, bob, Build Bot]   # handles, or an app's name for its bot
        messages:
          - user: bob
            text: nightly is red
            reactions: {eyes: [alice]}
            replies:
              - {user: Build Bot, text: Rerunning job 4411}
          - user: alice
            blocks: [{type: section, text: {type: mrkdwn, text: "*Deploy* today?"}}]
```

The response maps each workspace domain to what was made:

```json
{
  "ok": true,
  "workspaces": {
    "acme": {
      "id": "T…", "name": "Acme Inc",
      "users": {"alice": "U…", "bob": "U…"},
      "channels": {"builds": "C…"},
      "apps": {
        "Build Bot": {
          "app_id": "A…", "bot_user_id": "U…", "bot_id": "B…",
          "bot_token": "xoxb-test-build", "app_token": "xapp-test-build",
          "credentials": {"signing_secret": "test-signing-secret", "verification_token": "…", "client_id": "…", "client_secret": "…"}
        }
      }
    }
  }
}
```

To load a fixture when the server starts, set `EMU_SEED_FILE` to its path.
It's applied only when the database has no users yet, so restarts don't
duplicate anything.

A fixture that can't be loaded fails with `invalid_fixture` (bad YAML or
unknown fields) or `seed_failed`. The message says where, e.g.
`channel #builds: no user or app named "carol"`. Seeding isn't
transactional, so after a failure, reset before trying again.

## Inspector

Workspace owners and admins also get **Inspector** in the sidebar. It's a
live view of the same API calls, deliveries and events, limited to their
workspace, with filters, pause and expandable payloads.
