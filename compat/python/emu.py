"""A tiny client for the emulator's browser-facing endpoints, used to set up
workspaces, users and apps the way a person would in the web UI."""

import http.cookiejar
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

EMU_URL = os.environ.get("EMU_URL", "http://127.0.0.1:8080").rstrip("/")
API_URL = f"{EMU_URL}/api/"


class Person:
    """A signed-in browser session."""

    def __init__(self, email: str, password: str = "password123", name: str | None = None):
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar))
        me = self._post("/auth/signup", {"email": email, "password": password, "display_name": name or email})
        self.user_id = me["workspaces"][0]["user"]["id"]
        self.team_id = me["workspaces"][0]["team"]["id"]

    def _post(self, path: str, body: dict) -> dict:
        req = urllib.request.Request(
            EMU_URL + path,
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with self.opener.open(req) as res:
            return json.load(res)

    def api(self, method: str, **args) -> dict:
        r = self._post(f"/api/{method}", args)
        if not r.get("ok"):
            raise RuntimeError(f"{method} failed: {r}")
        return r

    def download(self, url: str) -> bytes:
        """Fetches a file URL (url_private) with this person's session."""
        with self.opener.open(url) as res:
            return res.read()

    def channel_id(self, name: str) -> str:
        chans = self.api("users.conversations", types="public_channel,private_channel")["channels"]
        return next(c["id"] for c in chans if c.get("name") == name)

    def wait_for_message(self, channel: str, predicate, timeout: float = 10, thread_ts: str | None = None) -> dict:
        """Polls history (or a thread) until a message matches."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if thread_ts:
                msgs = self.api("conversations.replies", channel=channel, ts=thread_ts)["messages"]
            else:
                msgs = self.api("conversations.history", channel=channel, limit=20)["messages"]
            for m in msgs:
                if predicate(m):
                    return m
            time.sleep(0.1)
        raise AssertionError(f"no matching message in {channel} within {timeout}s")


class Harness:
    """The emulator's test harness (`/_emu/*`), authenticated with EMU_ADMIN_TOKEN."""

    def __init__(self, token: str | None = None):
        self.token = token or os.environ["EMU_ADMIN_TOKEN"]

    def _call(self, method: str, path: str, body=None, content_type: str = "application/json"):
        data = None
        if body is not None:
            data = body.encode() if isinstance(body, str) else json.dumps(body).encode()
        req = urllib.request.Request(
            f"{EMU_URL}/_emu{path}",
            data=data,
            method=method,
            headers={"Authorization": f"Bearer {self.token}", "Content-Type": content_type},
        )
        try:
            with urllib.request.urlopen(req) as res:
                return res.status, res.headers, json.load(res)
        except urllib.error.HTTPError as e:
            return e.code, e.headers, json.load(e)

    def reset(self) -> dict:
        return self._call("POST", "/reset", {})[2]

    def seed(self, fixture: str | dict) -> dict:
        """Loads a fixture (YAML text or a dict) and returns the IDs and tokens it created."""
        content_type = "application/yaml" if isinstance(fixture, str) else "application/json"
        status, _, body = self._call("POST", "/seed", fixture, content_type)
        assert status == 200, body
        return body

    def act(self, who: str, method: str, **args) -> tuple[dict, int]:
        """Calls a Web API method as a person. Returns the response and the event seq to wait after."""
        _, headers, body = self._call("POST", f"/as/{who}/{method}", args)
        return body, int(headers["X-Emu-Seq"])

    def wait(self, after: int = 0, timeout: int = 10, **filters) -> dict:
        """The first event after `after` matching the filters (e.g. type, channel, user, text)."""
        query = urllib.parse.urlencode({"after": after, "timeout": timeout, **filters})
        status, _, body = self._call("GET", f"/wait?{query}")
        assert status == 200, f"no event matching {filters} within {timeout}s"
        return body["event"]["event"]

    def api_log(self, **filters) -> list[dict]:
        return self._call("GET", f"/api-log?{urllib.parse.urlencode(filters)}")[2]["entries"]

    def rate_limits(self, **config) -> dict:
        return self._call("POST", "/rate-limits", config)[2]


def unique_email(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}@example.com"


def wait_until(fn, timeout: float = 10, what: str = "condition"):
    """Polls `fn` until it returns something truthy, and returns that."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        got = fn()
        if got:
            return got
        time.sleep(0.1)
    raise AssertionError(f"timed out waiting for {what}")


def manifest(name: str, *, socket_mode: bool, request_url: str | None = None, commands: tuple[str, ...] = ()) -> dict:
    """An app with events, interactivity, slash commands and a Home tab. HTTP
    apps send everything to `request_url` (Bolt serves it all on one path)."""
    events = {"bot_events": ["message.channels", "app_mention", "app_home_opened"]}
    interactivity = {"is_enabled": True}
    if request_url:
        events["request_url"] = request_url
        interactivity["request_url"] = request_url
    slash_commands = [
        {"command": c, "description": f"Test command {c}", **({"url": request_url} if request_url else {})}
        for c in commands
    ]
    return {
        "display_information": {"name": name},
        "features": {
            "bot_user": {"display_name": name, "always_online": True},
            "app_home": {"home_tab_enabled": True, "messages_tab_enabled": True},
            "slash_commands": slash_commands,
        },
        "oauth_config": {
            "scopes": {
                "bot": [
                    "chat:write",
                    "channels:history",
                    "channels:read",
                    "app_mentions:read",
                    "users:read",
                    "commands",
                    "files:write",
                    "files:read",
                ]
            }
        },
        "settings": {"event_subscriptions": events, "interactivity": interactivity, "socket_mode_enabled": socket_mode},
    }
