//! Operations shared by the Web API and browser auth flows: workspace setup,
//! joining and leaving conversations, and posting messages (with the system
//! messages and events Slack generates for each).

use serde_json::json;
use slack_emu_core::models::{Conversation, Message, NewConversation, User};
use slack_emu_core::now;
use slack_emu_store::StoreError;

use crate::AppState;
use crate::api::ApiResult;
use crate::api::shapes::{self, ConvExtra};
use crate::events::{Audience, publish, publish_to_members};

/// Makes sure the workspace has `#general` and `#random`.
pub async fn ensure_workspace_defaults(state: &AppState, team_id: &str) -> ApiResult<()> {
    if state.store.general_conversation(team_id).await?.is_none() {
        let mut general = create_channel(state, team_id, "general", false, None, true).await?;
        general.purpose.value =
            "This is the one channel that will always include everyone. It’s a great spot for announcements and \
             team-wide conversations."
                .into();
        general.purpose.last_set = now();
        state.store.update_conversation(&general).await?;
    }
    if state.store.conversation_by_name(team_id, "random").await?.is_none() {
        create_channel(state, team_id, "random", false, None, false).await?;
    }
    Ok(())
}

async fn create_channel(
    state: &AppState,
    team_id: &str,
    name: &str,
    is_private: bool,
    creator: Option<&str>,
    is_general: bool,
) -> ApiResult<Conversation> {
    let members: Vec<String> = creator.map(|c| vec![c.to_owned()]).unwrap_or_default();
    Ok(state
        .store
        .create_conversation(
            NewConversation {
                team_id: team_id.into(),
                name: Some(name.into()),
                is_private,
                is_im: false,
                is_mpim: false,
                is_general,
                creator: creator.map(Into::into),
                dm_key: None,
            },
            &members,
        )
        .await?)
}

/// Creates a channel on behalf of a user and announces it.
pub async fn create_channel_as(
    state: &AppState,
    creator: &User,
    name: &str,
    is_private: bool,
) -> Result<Conversation, CreateError> {
    let conv = match state
        .store
        .create_conversation(
            NewConversation {
                team_id: creator.team_id.clone(),
                name: Some(name.into()),
                is_private,
                is_im: false,
                is_mpim: false,
                is_general: false,
                creator: Some(creator.id.clone()),
                dm_key: None,
            },
            std::slice::from_ref(&creator.id),
        )
        .await
    {
        Err(StoreError::Conflict(_)) => return Err(CreateError::NameTaken),
        other => other.map_err(|e| CreateError::Api(e.into()))?,
    };
    let summary = json!({
        "id": conv.id,
        "name": conv.name,
        "created": conv.created,
        "creator": creator.id,
        "is_private": is_private,
    });
    if is_private {
        // Private channels are only announced to their members.
        publish(
            state,
            &conv.team_id,
            Audience::Users(vec![creator.id.clone()]),
            false,
            json!({"type": "group_created", "channel": summary}),
        );
    } else {
        publish(
            state,
            &conv.team_id,
            Audience::Team,
            false,
            json!({"type": "channel_created", "channel": summary}),
        );
    }
    Ok(conv)
}

pub enum CreateError {
    NameTaken,
    Api(crate::api::ApiError),
}

/// Called when a human joins a workspace: announces them and adds them to the
/// default channels.
pub async fn on_workspace_join(state: &AppState, user: &User) -> ApiResult<()> {
    publish(
        state,
        &user.team_id,
        Audience::Team,
        false,
        json!({"type": "team_join", "user": shapes::user(user)}),
    );
    if let Some(general) = state.store.general_conversation(&user.team_id).await? {
        join_conversation(state, &general, &user.id, None).await?;
    }
    if let Some(random) = state.store.conversation_by_name(&user.team_id, "random").await?
        && !random.is_archived
        && !random.is_private
    {
        join_conversation(state, &random, &user.id, None).await?;
    }
    Ok(())
}

/// `C` for public channels, `G` for private ones, as in `member_joined_channel`.
fn short_channel_type(c: &Conversation) -> &'static str {
    if c.is_private { "G" } else { "C" }
}

/// Adds a member, posts the `channel_join` message, and emits events.
/// Returns false if they were already a member.
pub async fn join_conversation(
    state: &AppState,
    conv: &Conversation,
    user_id: &str,
    inviter: Option<&str>,
) -> ApiResult<bool> {
    if !state.store.add_member(&conv.id, user_id).await? {
        return Ok(false);
    }
    // The joiner's client needs the conversation itself.
    let joined_type = if conv.is_private {
        "group_joined"
    } else {
        "channel_joined"
    };
    let obj = shapes::conversation(
        conv,
        &ConvExtra {
            viewer: Some(user_id.into()),
            is_member: Some(true),
            ..Default::default()
        },
    );
    publish(
        state,
        &conv.team_id,
        Audience::Users(vec![user_id.into()]),
        true,
        json!({"type": joined_type, "channel": obj}),
    );
    if conv.is_channel() {
        let mut ev = json!({
            "type": "member_joined_channel",
            "user": user_id,
            "channel": conv.id,
            "channel_type": short_channel_type(conv),
            "team": conv.team_id,
        });
        if let Some(i) = inviter {
            ev["inviter"] = json!(i);
        }
        publish_to_members(state, conv, &[], false, ev).await?;
        let text = match inviter {
            Some(i) if i != user_id => format!("<@{user_id}> has joined the channel (invited by <@{i}>)"),
            _ => format!("<@{user_id}> has joined the channel"),
        };
        post_system_message(state, conv, user_id, "channel_join", &text).await?;
    }
    Ok(true)
}

/// Removes a member, posts the `channel_leave` message, and emits events.
/// Returns false if they weren't a member.
pub async fn leave_conversation(state: &AppState, conv: &Conversation, user_id: &str) -> ApiResult<bool> {
    if state.store.member_last_read(&conv.id, user_id).await?.is_none() {
        return Ok(false);
    }
    // Post while still a member so the leaver sees it too.
    if conv.is_channel() {
        post_system_message(
            state,
            conv,
            user_id,
            "channel_leave",
            &format!("<@{user_id}> has left the channel"),
        )
        .await?;
    }
    state.store.remove_member(&conv.id, user_id).await?;
    if conv.is_channel() {
        publish_to_members(
            state,
            conv,
            &[user_id],
            false,
            json!({
                "type": "member_left_channel",
                "user": user_id,
                "channel": conv.id,
                "channel_type": short_channel_type(conv),
                "team": conv.team_id,
            }),
        )
        .await?;
    }
    let left_type = if conv.is_private { "group_left" } else { "channel_left" };
    publish(
        state,
        &conv.team_id,
        Audience::Users(vec![user_id.into()]),
        false,
        json!({"type": left_type, "channel": conv.id, "actor_id": user_id}),
    );
    Ok(true)
}

/// Posts a system message such as `channel_join` or `channel_topic`.
pub async fn post_system_message(
    state: &AppState,
    conv: &Conversation,
    user_id: &str,
    subtype: &str,
    text: &str,
) -> ApiResult<Message> {
    let msg = Message {
        channel_id: conv.id.clone(),
        ts: state.clock.next(),
        team_id: conv.team_id.clone(),
        user_id: Some(user_id.into()),
        bot_id: None,
        app_id: None,
        subtype: Some(subtype.into()),
        text: text.into(),
        blocks: None,
        attachments: None,
        username: None,
        icons: None,
        metadata: None,
        client_msg_id: None,
        thread_ts: None,
        reply_broadcast: false,
        edited_ts: None,
        edited_by: None,
    };
    state.store.insert_message(&msg).await?;
    announce_message(state, conv, &msg).await?;
    Ok(msg)
}

/// Emits the `message` event for a newly stored message, refreshes its thread
/// parent for clients, and marks it read for its author.
pub async fn announce_message(state: &AppState, conv: &Conversation, msg: &Message) -> ApiResult<()> {
    let mut ev = shapes::one_message(state, msg).await?;
    ev["channel"] = json!(conv.id);
    ev["channel_type"] = json!(conv.channel_type());
    ev["event_ts"] = json!(msg.ts);
    publish_to_members(state, conv, &[], false, ev).await?;

    if msg.is_reply() {
        thread_updated(state, conv, msg.thread_ts.as_deref().expect("reply")).await?;
    }
    if let Some(author) = &msg.user_id
        && let Some(last_read) = state.store.member_last_read(&conv.id, author).await?
        && last_read < msg.ts
        && !msg.is_reply()
    {
        state.store.set_last_read(&conv.id, author, &msg.ts).await?;
    }
    Ok(())
}

/// Tells clients a thread parent's reply summary changed (UI only).
pub async fn thread_updated(state: &AppState, conv: &Conversation, parent_ts: &str) -> ApiResult<()> {
    if let Some(parent) = state.store.get_message(&conv.id, parent_ts).await? {
        let msg = shapes::one_message(state, &parent).await?;
        publish_to_members(
            state,
            conv,
            &[],
            true,
            json!({"type": "emu_thread_updated", "channel": conv.id, "message": msg}),
        )
        .await?;
    }
    Ok(())
}
