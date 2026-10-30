//! Behaviour every backend must share. Each backend's tests run this whole
//! suite; Postgres only when `EMU_TEST_POSTGRES` names a server (each test
//! gets a fresh database there).

use slack_emu_core::models::{
    Bookmark, Conversation, File, FileShare, Message, NewConversation, NewUser, Pin, ScheduledMessage, Usergroup,
};
use slack_emu_core::now;

use crate::{ConversationQuery, MessageSearch, Store, StoreError, TsRange};

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
        file_ids: Vec::new(),
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

pub(crate) async fn files(s: &dyn Store) {
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    let a = s.create_user(new_user(&ws.id, "a")).await.unwrap();
    let c = s
        .create_conversation(channel(&ws.id, "chat", false), std::slice::from_ref(&a.id))
        .await
        .unwrap();
    let mut f = File {
        id: "F1".into(),
        team_id: ws.id.clone(),
        user_id: a.id.clone(),
        bot_id: None,
        name: "a.bin".into(),
        title: "a.bin".into(),
        mimetype: "application/octet-stream".into(),
        filetype: "binary".into(),
        snippet_type: None,
        alt_txt: None,
        size: 4,
        created: now(),
        upload_token: Some("tok".into()),
        uploaded: false,
        completed: false,
    };
    s.create_file(&f).await.unwrap();
    assert_eq!(s.file_by_upload_token("tok").await.unwrap().unwrap().id, "F1");
    // Arbitrary bytes round-trip (BLOB / BYTEA).
    let bytes = [0u8, 255, 10, 13];
    s.set_file_data("F1", &bytes).await.unwrap();
    assert_eq!(s.file_data("F1").await.unwrap().unwrap(), bytes);
    assert!(
        s.list_files(&ws.id).await.unwrap().is_empty(),
        "incomplete files aren't listed"
    );

    f.upload_token = None;
    f.uploaded = true;
    f.completed = true;
    f.title = "Report".into();
    s.update_file(&f).await.unwrap();
    assert_eq!(s.get_file("F1").await.unwrap().unwrap(), f);
    assert_eq!(s.list_files(&ws.id).await.unwrap(), vec![f.clone()]);

    let mut m = msg(&c.id, "1000000001.000000", &a.id, "here", None);
    m.subtype = Some("file_share".into());
    m.file_ids = vec!["F1".into()];
    s.insert_message(&m).await.unwrap();
    let share = FileShare {
        file_id: "F1".into(),
        channel_id: c.id.clone(),
        ts: m.ts.clone(),
    };
    s.add_file_share(&share).await.unwrap();
    s.add_file_share(&share).await.unwrap();
    assert_eq!(s.file_shares(&["F1".into()]).await.unwrap(), vec![share]);
    assert_eq!(s.get_files(&["F1".into(), "F2".into()]).await.unwrap().len(), 1);

    s.delete_file("F1").await.unwrap();
    assert!(s.get_file("F1").await.unwrap().is_none());
    assert!(s.file_data("F1").await.unwrap().is_none());
    assert!(s.file_shares(&["F1".into()]).await.unwrap().is_empty());
    // The message still names the file, so it can show a tombstone.
    assert_eq!(s.get_message(&c.id, &m.ts).await.unwrap().unwrap().file_ids, ["F1"]);
}

pub(crate) async fn pins_and_bookmarks(s: &dyn Store) {
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    let a = s.create_user(new_user(&ws.id, "a")).await.unwrap();
    let c = s
        .create_conversation(channel(&ws.id, "chat", false), std::slice::from_ref(&a.id))
        .await
        .unwrap();
    for ts in ["1000000001.000000", "1000000002.000000"] {
        s.insert_message(&msg(&c.id, ts, &a.id, "x", None)).await.unwrap();
    }
    let pin = |ts: &str| Pin {
        channel_id: c.id.clone(),
        ts: ts.into(),
        pinned_by: a.id.clone(),
        created: now(),
    };
    assert!(s.add_pin(&pin("1000000002.000000")).await.unwrap());
    assert!(!s.add_pin(&pin("1000000002.000000")).await.unwrap());
    assert!(s.add_pin(&pin("1000000001.000000")).await.unwrap());
    assert_eq!(s.pins(&c.id).await.unwrap().len(), 2);
    assert_eq!(
        s.pinned(&c.id, &["1000000001.000000".into(), "1000000009.000000".into()])
            .await
            .unwrap(),
        ["1000000001.000000"]
    );
    assert!(s.remove_pin(&c.id, "1000000001.000000").await.unwrap());
    assert!(!s.remove_pin(&c.id, "1000000001.000000").await.unwrap());
    // Deleting a message unpins it.
    s.delete_message(&c.id, "1000000002.000000").await.unwrap();
    assert!(s.pins(&c.id).await.unwrap().is_empty());

    let mut b = Bookmark {
        id: "Bk1".into(),
        channel_id: c.id.clone(),
        title: "Runbook".into(),
        link: "https://example.com/runbook".into(),
        emoji: Some(":book:".into()),
        app_id: None,
        created: now(),
        updated: now(),
        updated_by: a.id.clone(),
    };
    s.add_bookmark(&b).await.unwrap();
    b.title = "Runbook v2".into();
    b.emoji = None;
    s.update_bookmark(&b).await.unwrap();
    assert_eq!(s.get_bookmark("Bk1").await.unwrap().unwrap(), b);
    assert_eq!(s.bookmarks(&c.id).await.unwrap(), vec![b]);
    s.remove_bookmark("Bk1").await.unwrap();
    assert!(s.bookmarks(&c.id).await.unwrap().is_empty());
}

pub(crate) async fn scheduled_messages(s: &dyn Store) {
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    let a = s.create_user(new_user(&ws.id, "a")).await.unwrap();
    let c = s
        .create_conversation(channel(&ws.id, "chat", false), std::slice::from_ref(&a.id))
        .await
        .unwrap();
    let sched = |id: &str, post_at: i64| ScheduledMessage {
        id: id.into(),
        team_id: ws.id.clone(),
        channel_id: c.id.clone(),
        user_id: a.id.clone(),
        token: Some("xoxb-x".into()),
        app_id: Some("A1".into()),
        post_at,
        created: 100,
        args: serde_json::json!({"text": id}),
    };
    s.add_scheduled(&sched("Q2", 300)).await.unwrap();
    s.add_scheduled(&sched("Q1", 200)).await.unwrap();
    assert_eq!(s.get_scheduled("Q1").await.unwrap().unwrap(), sched("Q1", 200));
    let ids = |v: Vec<ScheduledMessage>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(s.list_scheduled(&ws.id).await.unwrap()), ["Q1", "Q2"]);
    assert_eq!(ids(s.due_scheduled(250).await.unwrap()), ["Q1"]);
    assert!(s.delete_scheduled("Q1").await.unwrap());
    assert!(!s.delete_scheduled("Q1").await.unwrap());
    assert_eq!(ids(s.due_scheduled(1000).await.unwrap()), ["Q2"]);
}

pub(crate) async fn usergroups(s: &dyn Store) {
    let ws = s.create_workspace("Acme", "acme").await.unwrap();
    let a = s.create_user(new_user(&ws.id, "a")).await.unwrap();
    let b = s.create_user(new_user(&ws.id, "b")).await.unwrap();
    let mut g = Usergroup {
        id: "S1".into(),
        team_id: ws.id.clone(),
        name: "Oncall".into(),
        handle: "oncall".into(),
        description: String::new(),
        channels: Vec::new(),
        created_by: a.id.clone(),
        updated_by: a.id.clone(),
        created: 1,
        updated: 1,
        disabled_at: None,
        deleted_by: None,
    };
    s.create_usergroup(&g).await.unwrap();
    let mut dup = g.clone();
    dup.id = "S2".into();
    dup.name = "Other".into();
    assert!(
        matches!(s.create_usergroup(&dup).await, Err(StoreError::Conflict(_))),
        "handles are unique"
    );
    g.description = "Pager rotation".into();
    g.channels = vec!["C1".into(), "C2".into()];
    g.disabled_at = Some(5);
    s.update_usergroup(&g).await.unwrap();
    assert_eq!(s.get_usergroup("S1").await.unwrap().unwrap(), g);
    assert_eq!(s.list_usergroups(&ws.id).await.unwrap(), vec![g.clone()]);
    s.set_usergroup_members("S1", &[b.id.clone(), a.id.clone()])
        .await
        .unwrap();
    let mut both = vec![a.id.clone(), b.id.clone()];
    both.sort();
    assert_eq!(s.usergroup_members("S1").await.unwrap(), both);
    s.set_usergroup_members("S1", std::slice::from_ref(&b.id))
        .await
        .unwrap();
    assert_eq!(s.usergroup_members("S1").await.unwrap(), std::slice::from_ref(&b.id));

    // A message mentioning an enabled group counts as a mention for its members.
    g.disabled_at = None;
    s.update_usergroup(&g).await.unwrap();
    let c = s
        .create_conversation(channel(&ws.id, "chat", false), &[a.id.clone(), b.id.clone()])
        .await
        .unwrap();
    s.insert_message(&msg(
        &c.id,
        "1000000001.000000",
        &a.id,
        "<!subteam^S1|@oncall> help",
        None,
    ))
    .await
    .unwrap();
    assert_eq!(s.counts_for_user(&b.id).await.unwrap()[0].mention_count, 1);
}

pub(crate) async fn search(s: &dyn Store) {
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
    s.insert_message(&msg(
        &public.id,
        "1000000001.000000",
        &a.id,
        "Deploy FAILED at 50%",
        None,
    ))
    .await
    .unwrap();
    s.insert_message(&msg(&public.id, "1000000002.000000", &b.id, "deploy fixed", None))
        .await
        .unwrap();
    s.insert_message(&msg(&secret.id, "1000000003.000000", &a.id, "secret deploy", None))
        .await
        .unwrap();
    let mut join = msg(&public.id, "1000000004.000000", &b.id, "deploy has joined", None);
    join.subtype = Some("channel_join".into());
    s.insert_message(&join).await.unwrap();

    fn q<'a>(team: &'a str, viewer: &'a str, terms: &[&str]) -> MessageSearch<'a> {
        MessageSearch {
            team_id: team,
            viewer,
            terms: terms.iter().map(|t| t.to_string()).collect(),
            channel_id: None,
            user_id: None,
            after: None,
            before: None,
            limit: 100,
        }
    }
    let texts = |v: Vec<Message>| v.into_iter().map(|m| m.text).collect::<Vec<_>>();
    // Case-insensitive, newest first, no system messages; private only for members.
    assert_eq!(
        texts(s.search_messages(&q(&ws.id, &a.id, &["DEPLOY"])).await.unwrap()),
        ["secret deploy", "deploy fixed", "Deploy FAILED at 50%"]
    );
    assert_eq!(
        texts(s.search_messages(&q(&ws.id, &b.id, &["deploy"])).await.unwrap()),
        ["deploy fixed", "Deploy FAILED at 50%"]
    );
    // All terms must match; wildcards are literal.
    assert_eq!(
        texts(s.search_messages(&q(&ws.id, &a.id, &["deploy", "50%"])).await.unwrap()),
        ["Deploy FAILED at 50%"]
    );
    assert!(s.search_messages(&q(&ws.id, &a.id, &["5_%"])).await.unwrap().is_empty());
    let mut filtered = q(&ws.id, &a.id, &["deploy"]);
    filtered.user_id = Some(b.id.clone());
    assert_eq!(texts(s.search_messages(&filtered).await.unwrap()), ["deploy fixed"]);
    let mut dated = q(&ws.id, &a.id, &["deploy"]);
    dated.channel_id = Some(public.id.clone());
    dated.before = Some("1000000002.000000".into());
    assert_eq!(
        texts(s.search_messages(&dated).await.unwrap()),
        ["Deploy FAILED at 50%"]
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
        async fn files() {
            let Some(s) = $make else { return };
            $crate::conformance::files(&s).await;
        }
        #[tokio::test]
        async fn pins_and_bookmarks() {
            let Some(s) = $make else { return };
            $crate::conformance::pins_and_bookmarks(&s).await;
        }
        #[tokio::test]
        async fn scheduled_messages() {
            let Some(s) = $make else { return };
            $crate::conformance::scheduled_messages(&s).await;
        }
        #[tokio::test]
        async fn usergroups() {
            let Some(s) = $make else { return };
            $crate::conformance::usergroups(&s).await;
        }
        #[tokio::test]
        async fn search() {
            let Some(s) = $make else { return };
            $crate::conformance::search(&s).await;
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
