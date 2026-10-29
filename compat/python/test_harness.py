"""The test harness, used the way a bot's CI would: load a fixture with fixed
tokens, run the bot, act as a person, and wait for the bot's reply. Also
forces a rate limit to check the SDK retries through it.

Runs last (it resets the emulator)."""

from slack_bolt import App
from slack_bolt.adapter.socket_mode import SocketModeHandler
from slack_sdk import WebClient
from slack_sdk.http_retry.builtin_handlers import RateLimitErrorRetryHandler

from emu import API_URL, Harness, wait_until

FIXTURE = """
workspaces:
  - domain: acme
    name: Acme
    users:
      - {name: alice, real_name: Alice Anders, email: alice@acme.test, password: password123, owner: true}
      - {name: bob}
    apps:
      - manifest:
          display_information: {name: Build Bot}
          features: {bot_user: {display_name: Build Bot}}
          oauth_config: {scopes: {bot: [chat:write, channels:history, channels:read]}}
          settings: {socket_mode_enabled: true, event_subscriptions: {bot_events: [message.channels]}}
        bot_token: xoxb-test-build
        app_token: xapp-test-build
    channels:
      - name: builds
        members: [alice, bob, Build Bot]
        messages:
          - {user: bob, text: "nightly is red"}
"""


def test_fixture_bot_and_rate_limit_retry():
    h = Harness()
    h.reset()
    acme = h.seed(FIXTURE)["workspaces"]["acme"]
    builds = acme["channels"]["builds"]
    app = acme["apps"]["Build Bot"]
    assert app["bot_token"] == "xoxb-test-build"

    client = WebClient(
        token="xoxb-test-build",
        base_url=API_URL,
        retry_handlers=[RateLimitErrorRetryHandler(max_retry_count=2)],
    )
    bolt = App(client=client, request_verification_enabled=False)

    @bolt.message("status")
    def status(say):
        say("all green")

    handler = SocketModeHandler(bolt, "xapp-test-build", web_client=WebClient(base_url=API_URL))
    handler.connect()
    try:
        wait_until(
            lambda: h.act("alice", "emu.apps.info", app_id=app["app_id"])[0]["app"]["socket_connections"] > 0,
            what="a Socket Mode connection",
        )
        # The bot's first reply is rate limited; slack_sdk waits Retry-After and retries.
        h.rate_limits(force={"chat.postMessage": {"count": 1, "retry_after": 1}})
        _, seq = h.act("alice", "chat.postMessage", channel=builds, text="status?")
        reply = h.wait(after=seq, type="message", channel=builds, user=app["bot_user_id"], text="all green")
        assert reply["bot_id"] == app["bot_id"]

        # A call is logged when it returns, which can be just after its event arrives.
        def posts():
            statuses = [e["status"] for e in h.api_log(method="chat.postMessage", via="token")]
            return statuses if len(statuses) >= 2 else None

        statuses = wait_until(posts, what="both chat.postMessage calls in the API log")
        assert statuses == [429, 200], statuses

        # Seeded history is there for the bot to read.
        history = client.conversations_history(channel=builds)["messages"]
        assert any(m["text"] == "nightly is red" for m in history)
    finally:
        handler.close()
