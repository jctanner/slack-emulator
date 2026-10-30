//! Slack app manifests (the subset the emulator understands). Unknown fields
//! are kept in the stored JSON and round-trip through `apps.manifest.export`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    #[serde(default)]
    pub display_information: DisplayInformation,
    #[serde(default)]
    pub features: Features,
    #[serde(default)]
    pub oauth_config: OauthConfig,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DisplayInformation {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Features {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_user: Option<BotUser>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slash_commands: Vec<SlashCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_home: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcuts: Vec<Shortcut>,
}

/// A global shortcut (from the shortcuts menu) or message shortcut (from a message's actions).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Shortcut {
    #[serde(default)]
    pub name: String,
    /// `global` or `message`.
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub callback_id: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BotUser {
    pub display_name: String,
    #[serde(default)]
    pub always_online: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SlashCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_hint: Option<String>,
    #[serde(default)]
    pub should_escape: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OauthConfig {
    #[serde(default)]
    pub scopes: Scopes,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirect_urls: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Scopes {
    #[serde(default)]
    pub bot: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_subscriptions: Option<EventSubscriptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactivity: Option<Interactivity>,
    #[serde(default)]
    pub socket_mode_enabled: bool,
    #[serde(default)]
    pub org_deploy_enabled: bool,
    #[serde(default)]
    pub token_rotation_enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EventSubscriptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_url: Option<String>,
    #[serde(default)]
    pub bot_events: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_events: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Interactivity {
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_menu_options_url: Option<String>,
}

/// A manifest validation problem, in Slack's `{message, pointer}` shape.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManifestError {
    pub message: String,
    pub pointer: String,
}

/// Bot scopes the emulator knows. Unknown scopes are rejected like Slack does.
pub const BOT_SCOPES: &[&str] = &[
    "app_mentions:read",
    "bookmarks:read",
    "bookmarks:write",
    "calls:read",
    "calls:write",
    "channels:history",
    "channels:join",
    "channels:manage",
    "channels:read",
    "channels:write.invites",
    "channels:write.topic",
    "chat:write",
    "chat:write.customize",
    "chat:write.public",
    "commands",
    "dnd:read",
    "emoji:read",
    "files:read",
    "files:write",
    "groups:history",
    "groups:read",
    "groups:write",
    "groups:write.invites",
    "groups:write.topic",
    "im:history",
    "im:read",
    "im:write",
    "incoming-webhook",
    "links:read",
    "links:write",
    "metadata.message:read",
    "mpim:history",
    "mpim:read",
    "mpim:write",
    "pins:read",
    "pins:write",
    "reactions:read",
    "reactions:write",
    "reminders:read",
    "reminders:write",
    "team:read",
    "usergroups:read",
    "usergroups:write",
    "users.profile:read",
    "users:read",
    "users:read.email",
    "users:write",
];

/// The scope a bot event subscription needs (any one of them), or `None` if
/// the event needs no scope.
pub fn event_scopes(event: &str) -> Option<&'static [&'static str]> {
    Some(match event {
        "message.channels" => &["channels:history"],
        "message.groups" => &["groups:history"],
        "message.im" => &["im:history"],
        "message.mpim" => &["mpim:history"],
        "app_mention" => &["app_mentions:read"],
        "reaction_added" | "reaction_removed" => &["reactions:read"],
        "member_joined_channel" | "member_left_channel" => &["channels:read", "groups:read"],
        "channel_created" | "channel_rename" | "channel_archive" | "channel_unarchive" | "channel_deleted"
        | "channel_left" => &["channels:read"],
        "group_rename" | "group_archive" | "group_unarchive" | "group_deleted" | "group_left" => &["groups:read"],
        "im_created" => &["im:read"],
        "file_shared" | "file_created" | "file_deleted" | "file_change" => &["files:read"],
        "pin_added" | "pin_removed" => &["pins:read"],
        "subteam_created"
        | "subteam_updated"
        | "subteam_members_changed"
        | "subteam_self_added"
        | "subteam_self_removed" => &["usergroups:read"],
        "team_join" | "user_change" => &["users:read"],
        "app_home_opened" | "app_uninstalled" | "tokens_revoked" | "app_rate_limited" => return None,
        _ => return Some(&[]),
    })
}

impl Manifest {
    pub fn bot_scopes(&self) -> &[String] {
        &self.oauth_config.scopes.bot
    }

    pub fn bot_events(&self) -> &[String] {
        self.settings
            .event_subscriptions
            .as_ref()
            .map(|e| e.bot_events.as_slice())
            .unwrap_or(&[])
    }

    pub fn request_url(&self) -> Option<&str> {
        self.settings
            .event_subscriptions
            .as_ref()
            .and_then(|e| e.request_url.as_deref())
            .filter(|u| !u.is_empty())
    }

    pub fn interactivity_url(&self) -> Option<&str> {
        self.settings
            .interactivity
            .as_ref()
            .filter(|i| i.is_enabled)
            .and_then(|i| i.request_url.as_deref())
            .filter(|u| !u.is_empty())
    }

    /// `features.app_home.<key>` as a boolean.
    fn app_home_flag(&self, key: &str) -> Option<bool> {
        self.features.app_home.as_ref()?.get(key)?.as_bool()
    }

    pub fn home_tab_enabled(&self) -> bool {
        self.app_home_flag("home_tab_enabled").unwrap_or(false)
    }

    /// The Messages tab is on unless turned off.
    pub fn messages_tab_enabled(&self) -> bool {
        self.app_home_flag("messages_tab_enabled").unwrap_or(true)
    }

    pub fn messages_tab_read_only(&self) -> bool {
        self.app_home_flag("messages_tab_read_only_enabled").unwrap_or(false)
    }

    pub fn interactivity_enabled(&self) -> bool {
        self.settings.interactivity.as_ref().is_some_and(|i| i.is_enabled)
    }

    /// Where `block_suggestion` requests go over HTTP.
    pub fn options_url(&self) -> Option<&str> {
        self.settings
            .interactivity
            .as_ref()
            .filter(|i| i.is_enabled)
            .and_then(|i| i.message_menu_options_url.as_deref())
            .filter(|u| !u.is_empty())
    }

    pub fn slash_command(&self, command: &str) -> Option<&SlashCommand> {
        self.features.slash_commands.iter().find(|c| c.command == command)
    }

    /// Checks the manifest the way `apps.manifest.validate` does.
    pub fn validate(&self) -> Vec<ManifestError> {
        let mut errs = Vec::new();
        let mut err = |pointer: &str, message: String| {
            errs.push(ManifestError {
                message,
                pointer: pointer.into(),
            })
        };
        let name = self.display_information.name.trim();
        if name.is_empty() {
            err("/display_information/name", "Your app needs a name.".into());
        } else if name.chars().count() > 35 {
            err(
                "/display_information/name",
                "Name must be 35 characters or fewer.".into(),
            );
        }
        for (i, s) in self.bot_scopes().iter().enumerate() {
            if !BOT_SCOPES.contains(&s.as_str()) {
                err(&format!("/oauth_config/scopes/bot/{i}"), format!("Invalid scope: {s}"));
            }
        }
        let has_bot = self.features.bot_user.is_some();
        if !self.bot_scopes().is_empty() && !has_bot {
            err(
                "/features/bot_user",
                "Bot scopes require a bot user (features.bot_user.display_name).".into(),
            );
        }
        if let Some(bot) = &self.features.bot_user
            && bot.display_name.trim().is_empty()
        {
            err(
                "/features/bot_user/display_name",
                "The bot needs a display name.".into(),
            );
        }
        for (i, ev) in self.bot_events().iter().enumerate() {
            match event_scopes(ev) {
                Some([]) => err(
                    &format!("/settings/event_subscriptions/bot_events/{i}"),
                    format!("Unknown event: {ev}"),
                ),
                Some(needed) if !needed.iter().any(|n| self.bot_scopes().iter().any(|s| s == n)) => err(
                    &format!("/settings/event_subscriptions/bot_events/{i}"),
                    format!("Event {ev} requires one of these bot scopes: {}", needed.join(", ")),
                ),
                _ => {}
            }
        }
        if let Some(url) = self.request_url()
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            err(
                "/settings/event_subscriptions/request_url",
                "Request URL must be an http(s) URL.".into(),
            );
        }
        for (i, cmd) in self.features.slash_commands.iter().enumerate() {
            let valid = cmd.command.starts_with('/')
                && cmd.command.len() > 1
                && cmd.command[1..]
                    .chars()
                    .all(|c| c.is_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
            if !valid {
                err(
                    &format!("/features/slash_commands/{i}/command"),
                    "Commands start with / and use lowercase letters, numbers, - and _.".into(),
                );
            }
            if !self.settings.socket_mode_enabled && cmd.url.as_deref().is_none_or(str::is_empty) {
                err(
                    &format!("/features/slash_commands/{i}/url"),
                    "Slash commands need a url unless Socket Mode is enabled.".into(),
                );
            }
        }
        for (i, sc) in self.features.shortcuts.iter().enumerate() {
            let at = |field: &str| format!("/features/shortcuts/{i}/{field}");
            if !matches!(sc.kind.as_str(), "global" | "message") {
                err(&at("type"), "Shortcut type must be global or message.".into());
            }
            if sc.name.trim().is_empty() || sc.name.chars().count() > 24 {
                err(&at("name"), "Shortcut names are 1 to 24 characters.".into());
            }
            if sc.callback_id.trim().is_empty() || sc.callback_id.chars().count() > 255 {
                err(
                    &at("callback_id"),
                    "Shortcuts need a callback_id (up to 255 characters).".into(),
                );
            }
            if sc.description.trim().is_empty() || sc.description.chars().count() > 150 {
                err(
                    &at("description"),
                    "Shortcut descriptions are 1 to 150 characters.".into(),
                );
            }
        }
        let needs_commands = !self.features.slash_commands.is_empty() || !self.features.shortcuts.is_empty();
        if needs_commands && !self.bot_scopes().iter().any(|s| s == "commands") {
            err(
                "/oauth_config/scopes/bot",
                "Slash commands and shortcuts require the commands scope.".into(),
            );
        }
        if !self.features.shortcuts.is_empty() && !self.interactivity_enabled() {
            err(
                "/settings/interactivity/is_enabled",
                "Shortcuts need interactivity turned on.".into(),
            );
        }
        if self.interactivity_enabled() && !self.settings.socket_mode_enabled && self.interactivity_url().is_none() {
            err(
                "/settings/interactivity/request_url",
                "Interactivity needs a request_url unless Socket Mode is enabled.".into(),
            );
        }
        errs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: Value) -> Manifest {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn validates() {
        let ok = parse(serde_json::json!({
            "display_information": {"name": "Echo"},
            "features": {"bot_user": {"display_name": "echo"}},
            "oauth_config": {"scopes": {"bot": ["chat:write", "channels:history"]}},
            "settings": {"event_subscriptions": {"bot_events": ["message.channels"]}, "socket_mode_enabled": true}
        }));
        assert!(ok.validate().is_empty(), "{:?}", ok.validate());
        assert!(ok.settings.socket_mode_enabled);

        let bad = parse(serde_json::json!({
            "oauth_config": {"scopes": {"bot": ["chat:write", "nope:nope"]}},
            "settings": {"event_subscriptions": {"bot_events": ["app_mention", "made_up"]}}
        }));
        let pointers: Vec<String> = bad.validate().into_iter().map(|e| e.pointer).collect();
        assert_eq!(
            pointers,
            [
                "/display_information/name",
                "/oauth_config/scopes/bot/1",
                "/features/bot_user",
                "/settings/event_subscriptions/bot_events/0",
                "/settings/event_subscriptions/bot_events/1",
            ]
        );
    }
}
