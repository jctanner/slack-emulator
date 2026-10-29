"""Real Bolt for Python apps against the emulator, over Socket Mode and HTTP."""

import socket
import threading
import time

from slack_bolt import App
from slack_bolt.adapter.socket_mode import SocketModeHandler
from slack_sdk import WebClient

from emu import API_URL, Person, manifest, unique_email, wait_until

TICKET_MODAL = {
    "type": "modal",
    "callback_id": "ticket",
    "title": {"type": "plain_text", "text": "New ticket"},
    "submit": {"type": "plain_text", "text": "Create"},
    "blocks": [
        {
            "type": "input",
            "block_id": "title",
            "label": {"type": "plain_text", "text": "Title"},
            "element": {"type": "plain_text_input", "action_id": "value"},
        }
    ],
}


def build_bolt(token: str, prefix: str, signing_secret: str | None = None) -> App:
    app = App(
        client=WebClient(token=token, base_url=API_URL),
        signing_secret=signing_secret,
        # Socket Mode apps have no signing secret; skip request verification there.
        request_verification_enabled=signing_secret is not None,
    )

    @app.message("ping")
    def pong(say):
        say("pong")

    @app.event("app_mention")
    def mention(event, say):
        say(text=f"hi <@{event['user']}>", thread_ts=event["ts"])

    @app.command(f"/{prefix}-echo")
    def echo(ack, respond, command):
        ack()
        respond(response_type="in_channel", text=f"echo: {command['text']}")

    @app.command(f"/{prefix}-ticket")
    def ticket_prompt(ack, say):
        ack()
        say(
            text="Need a ticket?",
            blocks=[
                {"type": "section", "text": {"type": "mrkdwn", "text": "Need a ticket?"}},
                {
                    "type": "actions",
                    "block_id": "ticket_actions",
                    "elements": [
                        {"type": "button", "action_id": "open_ticket", "text": {"type": "plain_text", "text": "Open"}}
                    ],
                },
            ],
        )

    @app.action("open_ticket")
    def open_ticket(ack, body, respond, client):
        ack()
        respond(replace_original=True, text="Opening a ticket…")
        client.views_open(trigger_id=body["trigger_id"], view={**TICKET_MODAL, "private_metadata": body["channel"]["id"]})

    @app.view("ticket")
    def create_ticket(ack, body, view, client):
        title = view["state"]["values"]["title"]["value"]["value"]
        if len(title) < 5:
            ack(response_action="errors", errors={"title": "Title must be at least 5 characters"})
            return
        ack()
        client.chat_postMessage(channel=view["private_metadata"], text=f"<@{body['user']['id']}> opened: {title}")

    @app.event("app_home_opened")
    def home(event, client):
        if event["tab"] == "home":
            client.views_publish(
                user_id=event["user"],
                view={
                    "type": "home",
                    "blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": f"Welcome home, <@{event['user']}>"}}],
                },
            )

    return app


def exercise(person: Person, bot_user: str, prefix: str):
    general = person.channel_id("general")
    person.api("conversations.invite", channel=general, users=bot_user)

    person.api("chat.postMessage", channel=general, text="ping")
    person.wait_for_message(general, lambda m: m.get("user") == bot_user and m["text"] == "pong")

    asked = person.api("chat.postMessage", channel=general, text=f"<@{bot_user}> hello there")
    reply = person.wait_for_message(
        general,
        lambda m: m.get("user") == bot_user and m["text"] == f"hi <@{person.user_id}>",
        thread_ts=asked["ts"],
    )
    assert reply["thread_ts"] == asked["ts"]
    assert reply["bot_profile"]["user_id"] == bot_user

    # Slash command answered through the response URL.
    person.api("chat.command", channel=general, command=f"/{prefix}-echo", text="hello")
    person.wait_for_message(general, lambda m: m.get("user") == bot_user and m["text"] == "echo: hello")

    # A button opens a modal; the app validates the submission.
    person.api("chat.command", channel=general, command=f"/{prefix}-ticket", text="")
    prompt = person.wait_for_message(general, lambda m: m.get("user") == bot_user and m["text"] == "Need a ticket?")
    person.api(
        "emu.blocks.action",
        container={"type": "message", "channel_id": general, "message_ts": prompt["ts"]},
        block_id="ticket_actions",
        action_id="open_ticket",
    )
    view = wait_until(lambda: (person.api("emu.views.current")["views"] or [None])[-1], what="the modal")
    assert view["callback_id"] == "ticket"
    replaced = person.wait_for_message(general, lambda m: m["ts"] == prompt["ts"] and m["text"] == "Opening a ticket…")
    assert "blocks" not in replaced

    def state(title):
        return {"values": {"title": {"value": {"type": "plain_text_input", "value": title}}}}

    rejected = person.api("emu.views.submit", view_id=view["id"], state=state("hi"))
    assert rejected["errors"] == {"title": "Title must be at least 5 characters"}
    person.api("emu.views.submit", view_id=view["id"], state=state("Broken build"))
    assert person.api("emu.views.current")["views"] == []
    person.wait_for_message(
        general, lambda m: m.get("user") == bot_user and m["text"] == f"<@{person.user_id}> opened: Broken build"
    )

    # App Home: opening the tab makes the app publish a view.
    dm = person.api("conversations.open", users=bot_user)["channel"]["id"]
    person.api("emu.apps.home", channel=dm, tab="home")
    home = wait_until(lambda: person.api("emu.apps.home", channel=dm)["view"], what="the Home tab")
    assert home["blocks"][0]["text"]["text"] == f"Welcome home, <@{person.user_id}>"


def test_socket_mode():
    alice = Person(unique_email("alice"))
    prefix = "py-sock"
    created = alice.api(
        "apps.manifest.create",
        manifest=manifest("Socket Bot", socket_mode=True, commands=(f"/{prefix}-echo", f"/{prefix}-ticket")),
    )
    app_id = created["app_id"]
    install = alice.api("emu.apps.install", app_id=app_id)
    app_token = alice.api("emu.apps.tokens.create", app_id=app_id)["token"]

    bolt = build_bolt(install["access_token"], prefix)
    handler = SocketModeHandler(bolt, app_token, web_client=WebClient(base_url=API_URL))
    handler.connect()
    try:
        wait_until(
            lambda: alice.api("emu.apps.info", app_id=app_id)["app"]["socket_connections"] > 0,
            what="a Socket Mode connection",
        )
        exercise(alice, install["bot_user_id"], prefix)
    finally:
        handler.close()


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def test_http_events():
    alice = Person(unique_email("alice"))
    prefix = "py-http"
    port = free_port()
    url = f"http://127.0.0.1:{port}/slack/events"
    # Bolt needs the credentials before it can answer url_verification, so
    # verification fails at creation and is retried once Bolt is up.
    spec = manifest("Http Bot", socket_mode=False, request_url=url, commands=(f"/{prefix}-echo", f"/{prefix}-ticket"))
    created = alice.api("apps.manifest.create", manifest=spec)
    app_id = created["app_id"]
    install = alice.api("emu.apps.install", app_id=app_id)

    bolt = build_bolt(install["access_token"], prefix, created["credentials"]["signing_secret"])
    threading.Thread(target=bolt.start, kwargs={"port": port, "path": "/slack/events"}, daemon=True).start()
    time.sleep(0.5)

    assert alice.api("emu.apps.verifyUrl", app_id=app_id)["verified"]
    assert alice.api("emu.apps.info", app_id=app_id)["app"]["request_url_verified"]
    exercise(alice, install["bot_user_id"], prefix)
