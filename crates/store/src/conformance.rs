//! Behaviour every backend must share. Each backend's tests run this whole
//! suite; Postgres only when `EMU_TEST_POSTGRES` names a server (each test
//! gets a fresh database there).

use slack_emu_core::models::{Conversation, Message, NewConversation, NewUser};
use slack_emu_core::now;

use crate::{ConversationQuery, Store, StoreError, TsRange};

pub(crate) async fn account_and_users(s: &dyn Store) {
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    assert!(matches!(
        s.create_workspace("Other", "acme").await,
        Err(StoreError::Conflict(_))
    ));

    let acct = s
        .create_account("a@example.com", "Alice", "password", "a@example.com", Some("h"))
        .await
        .unwrap();
    let found = s.find_identity("password", "a@example.com").await.unwrap().unwrap();
    assert_eq!(found.account, acct);
    assert_eq!(found.secret_hash.as_deref(), Some("h"));

    let user = s
        .create_user(NewUser {
            team_id: ws.id.clone(),
            account_id: Some(acct.id.clone()),
            name: "alice".into(),
            real_name: "Alice".into(),
            display_name: "Alice".into(),
            email: Some(acct.email.clone()),
            is_bot: false,
            is_admin: true,
            is_owner: true,
        })
        .await
        .unwrap();
    assert_eq!(s.user_for_account(&ws.id, &acct.id).await.unwrap(), Some(user.clone()));
    assert_eq!(s.users_for_account(&acct.id).await.unwrap(), vec![user]);
}

pub(crate) async fn sessions_expire(s: &dyn Store) {
    let acct = s
        .create_account("b@example.com", "Bob", "password", "b@example.com", None)
        .await
        .unwrap();
    s.create_session("live", &acct.id, now() + 60).await.unwrap();
    s.create_session("dead", &acct.id, now() - 1).await.unwrap();
    assert!(s.session_account("live").await.unwrap().is_some());
    assert!(s.session_account("dead").await.unwrap().is_none());
    s.delete_session("live").await.unwrap();
    assert!(s.session_account("live").await.unwrap().is_none());
}

fn new_user(team: &str, name: &str) -> NewUser {
    NewUser {
        team_id: team.into(),
        account_id: None,
        name: name.into(),
        real_name: name.into(),
        display_name: name.into(),
        email: None,
        is_bot: false,
        is_admin: false,
        is_owner: false,
    }
}

fn channel(team: &str, name: &str, private: bool) -> NewConversation {
    NewConversation {
        team_id: team.into(),
        name: Some(name.into()),
        is_private: private,
        is_im: false,
        is_mpim: false,
        is_general: false,
        creator: None,
        dm_key: None,
    }
}

fn msg(channel: &str, ts: &str, user: &str, text: &str, thread_ts: Option<&str>) -> Message {
    Message {
        channel_id: channel.into(),
        ts: ts.into(),
        team_id: "T".into(),
        user_id: Some(user.into()),
        bot_id: None,
        app_id: None,
        subtype: None,
        text: text.into(),
        blocks: None,
        attachments: None,
        username: None,
        icons: None,
        metadata: None,
        client_msg_id: None,
        thread_ts: thread_ts.map(Into::into),
        reply_broadcast: false,
        edited_ts: None,
        edited_by: None,
    }
}

pub(crate) async fn conversation_visibility(s: &dyn Store) {
    use slack_emu_core::models::ConversationTypes;
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    let a = s.create_user(new_user(&ws.id, "a")).await.unwrap();
    let b = s.create_user(new_user(&ws.id, "b")).await.unwrap();
    let public = s
        .create_conversation(channel(&ws.id, "pub", false), std::slice::from_ref(&a.id))
        .await
        .unwrap();
    let secret = s
        .create_conversation(channel(&ws.id, "secret", true), std::slice::from_ref(&a.id))
        .await
        .unwrap();
    assert!(matches!(
        s.create_conversation(channel(&ws.id, "pub", true), &[]).await,
        Err(StoreError::Conflict(_))
    ));

    fn q<'a>(team_id: &'a str, viewer: &'a str, member_only: bool) -> ConversationQuery<'a> {
        ConversationQuery {
            team_id,
            viewer,
            types: ConversationTypes {
                public: true,
                private: true,
                im: true,
                mpim: true,
            },
            exclude_archived: false,
            member_only,
            after_id: None,
            limit: 100,
        }
    }
    let ids = |v: Vec<Conversation>| v.into_iter().map(|c| c.id).collect::<Vec<_>>();
    let mut expected = vec![public.id.clone(), secret.id.clone()];
    expected.sort();
    assert_eq!(
        ids(s.list_conversations(q(&ws.id, &a.id, false)).await.unwrap()),
        expected
    );
    assert_eq!(
        ids(s.list_conversations(q(&ws.id, &b.id, false)).await.unwrap()),
        vec![public.id.clone()]
    );
    assert!(s.list_conversations(q(&ws.id, &b.id, true)).await.unwrap().is_empty());

    assert!(s.add_member(&secret.id, &b.id).await.unwrap());
    assert!(!s.add_member(&secret.id, &b.id).await.unwrap());
    assert_eq!(s.member_count(&secret.id).await.unwrap(), 2);
    assert!(s.remove_member(&secret.id, &b.id).await.unwrap());
    assert!(s.member_last_read(&secret.id, &b.id).await.unwrap().is_none());
}

pub(crate) async fn threads_reactions_and_counts(s: &dyn Store) {
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    let a = s.create_user(new_user(&ws.id, "a")).await.unwrap();
    let b = s.create_user(new_user(&ws.id, "b")).await.unwrap();
    let c = s
        .create_conversation(channel(&ws.id, "chat", false), &[a.id.clone(), b.id.clone()])
        .await
        .unwrap();

    s.insert_message(&msg(&c.id, "1000000001.000000", &a.id, "hello", None))
        .await
        .unwrap();
    s.insert_message(&msg(&c.id, "1000000002.000000", &b.id, "hi <@A>", None))
        .await
        .unwrap();
    s.insert_message(&msg(
        &c.id,
        "1000000003.000000",
        &b.id,
        "reply",
        Some("1000000001.000000"),
    ))
    .await
    .unwrap();
    let mut bcast = msg(&c.id, "1000000004.000000", &a.id, "bcast", Some("1000000001.000000"));
    bcast.reply_broadcast = true;
    bcast.subtype = Some("thread_broadcast".into());
    s.insert_message(&bcast).await.unwrap();

    let parent = s.get_message(&c.id, "1000000001.000000").await.unwrap().unwrap();
    assert!(parent.is_thread_parent());

    let hist: Vec<String> = s
        .history(&c.id, &TsRange::default(), 10)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.ts)
        .collect();
    assert_eq!(hist, ["1000000004.000000", "1000000002.000000", "1000000001.000000"]);

    let range = TsRange {
        latest: Some("1000000002.000000".into()),
        ..Default::default()
    };
    assert_eq!(s.history(&c.id, &range, 10).await.unwrap().len(), 1);

    let thread = s
        .replies(&c.id, "1000000001.000000", &TsRange::default(), 10)
        .await
        .unwrap();
    assert_eq!(thread.len(), 3);
    assert_eq!(thread[0].ts, "1000000001.000000");

    let sums = s.thread_summaries(&c.id, &["1000000001.000000".into()]).await.unwrap();
    let sum = &sums["1000000001.000000"];
    assert_eq!(sum.reply_count, 2);
    assert_eq!(sum.reply_users, vec![b.id.clone(), a.id.clone()]);
    assert_eq!(sum.latest_reply.as_deref(), Some("1000000004.000000"));

    assert!(s.add_reaction(&c.id, "1000000001.000000", "tada", &a.id).await.unwrap());
    assert!(s.add_reaction(&c.id, "1000000001.000000", "+1", &b.id).await.unwrap());
    assert!(s.add_reaction(&c.id, "1000000001.000000", "tada", &b.id).await.unwrap());
    assert!(!s.add_reaction(&c.id, "1000000001.000000", "tada", &b.id).await.unwrap());
    let r = s.reactions(&c.id, &["1000000001.000000".into()]).await.unwrap();
    let r = &r["1000000001.000000"];
    assert_eq!(r[0].name, "tada");
    assert_eq!(r[0].users, vec![a.id.clone(), b.id.clone()]);
    assert_eq!(r[1].name, "+1");

    // a hasn't read anything: b's top-level message is unread; the
    // non-broadcast reply and a's own broadcast don't count.
    let counts = s.counts_for_user(&a.id).await.unwrap();
    assert_eq!(counts[0].unread_count, 1);
    assert_eq!(counts[0].mention_count, 0);
    assert_eq!(counts[0].latest.as_deref(), Some("1000000004.000000"));
    s.set_last_read(&c.id, &a.id, "1000000004.000000").await.unwrap();
    assert_eq!(s.counts_for_user(&a.id).await.unwrap()[0].unread_count, 0);
    // b has a mention-free unread from a (the top-level hello and broadcast).
    assert_eq!(s.counts_for_user(&b.id).await.unwrap()[0].unread_count, 2);

    assert_eq!(s.max_ts().await.unwrap().as_deref(), Some("1000000004.000000"));
    s.delete_message(&c.id, "1000000001.000000").await.unwrap();
    assert!(
        s.reactions(&c.id, &["1000000001.000000".into()])
            .await
            .unwrap()
            .is_empty()
    );
}

/// Declares one `#[tokio::test]` per conformance check, each with a store
/// from `$make` (an `Option`: `None` skips, e.g. when no Postgres is configured).
macro_rules! suite {
    ($make:expr) => {
        #[tokio::test]
        async fn account_and_users() {
            let Some(s) = $make else { return };
            $crate::conformance::account_and_users(&s).await;
        }
        #[tokio::test]
        async fn conversation_visibility() {
            let Some(s) = $make else { return };
            $crate::conformance::conversation_visibility(&s).await;
        }
        #[tokio::test]
        async fn sessions_expire() {
            let Some(s) = $make else { return };
            $crate::conformance::sessions_expire(&s).await;
        }
        #[tokio::test]
        async fn threads_reactions_and_counts() {
            let Some(s) = $make else { return };
            $crate::conformance::threads_reactions_and_counts(&s).await;
        }
    };
}

pub(crate) use suite;
