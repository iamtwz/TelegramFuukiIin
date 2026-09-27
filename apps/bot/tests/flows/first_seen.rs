use super::*;
use fuuki_iin_bot::model::{FirstMessage, ScreeningMode, Settings};

fn select_mode(e: &Engine<Mock>, chat: i64, mode: ScreeningMode) {
    let mut settings = e.settings(chat).unwrap();
    settings.screening = mode;
    e.store.put(chat, "settings", &settings).unwrap();
}

#[tokio::test]
async fn non_member_comments_share_one_marker_across_posts_but_not_groups() {
    let (mut e, _) = setup(Some(0.1));
    let other = CHAT - 1;
    Arc::get_mut(&mut e.config).unwrap().chats.push(other);
    for chat in [CHAT, other] {
        select_mode(&e, chat, ScreeningMode::FirstSeen);
    }
    e.services.member(USER, json!({"status":"left"}));
    for (id, chat, root) in [(2010, CHAT, 100), (2011, CHAT, 200), (2012, other, 100)] {
        let mut comment = message(id);
        comment["chat"]["id"] = json!(chat);
        comment["message_thread_id"] = json!(root);
        comment["reply_to_message"] =
            json!({"message_id":root,"is_automatic_forward":true,"sender_chat":{"id":-100555}});
        let update = json!({"update_id":id,"message":comment});
        e.ingest(&update).unwrap();
        e.ingest(&update).unwrap();
    }
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 2);
    assert_eq!(cases(&e).len(), 1);
    assert_eq!(cases(&e)[0].message, 2010);
    assert_eq!(cases(&e)[0].screening, ScreeningMode::FirstSeen);
    assert_eq!(cases(&e)[0].joined_at, 0);
    // Observing a comment does not create membership or a verification session.
    assert!(
        e.store
            .get::<Member>(CHAT, &format!("member:{USER}"))
            .unwrap()
            .is_none()
    );
    assert!(
        e.store
            .list::<Session>(CHAT, "session:", None, 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
}

#[tokio::test]
async fn non_members_use_existing_thresholds_review_and_enforcement() {
    for (probability, expected) in [
        (0.599999, "allowed"),
        (0.6, "review"),
        (0.799999, "review"),
        (0.8, "banned"),
    ] {
        let (e, _) = setup(Some(probability));
        select_mode(&e, CHAT, ScreeningMode::FirstSeen);
        e.services.member(USER, json!({"status":"left"}));
        e.services.member(7, json!({"status":"creator"}));
        let mut comment = message(2020);
        comment["via_bot"] = json!({"id":88,"is_bot":true});
        comment["reply_markup"] =
            json!({"inline_keyboard":[[{"text":"快速致富","url":"https://example.invalid"}]]});
        e.message(CHAT, &comment).unwrap();
        drain(&e).await;
        assert_eq!(cases(&e)[0].state, expected);
        assert_eq!(
            e.services.evidence.lock().unwrap()[0]["message"]["reply_markup"],
            comment["reply_markup"]
        );
        if expected == "review" {
            assert!(
                e.services
                    .calls("sendMessage")
                    .iter()
                    .all(|r| r["chat_id"] == 7)
            );
            admin::handle(&e, CHAT, &admin_callback(7, CHAT, "ban", &cases(&e)[0].id))
                .await
                .unwrap();
            drain(&e).await;
            assert_eq!(cases(&e)[0].state, "banned");
        }
        if expected != "allowed" {
            assert_eq!(e.services.calls("deleteMessage")[0]["message_id"], 2020);
            assert_eq!(e.services.calls("banChatMember")[0]["user_id"], USER);
        } else {
            assert!(e.services.calls("banChatMember").is_empty());
        }
    }
}

#[tokio::test]
async fn first_seen_includes_former_members_and_preserves_admin_protection() {
    let (e, _) = setup(Some(0.99));
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &json!({"date":NOW,"left_chat_member":{"id":USER}}))
        .unwrap();
    e.services.member(USER, json!({"status":"left"}));
    e.message(CHAT, &message(2030)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "banned");
    assert!(
        !e.store
            .get::<Member>(CHAT, &format!("member:{USER}"))
            .unwrap()
            .unwrap()
            .active
    );

    let (e, _) = setup(Some(0.99));
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.services.member(USER, json!({"status":"administrator"}));
    e.message(CHAT, &message(2031)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "protected");
    assert!(e.services.evidence.lock().unwrap().is_empty());
    assert!(e.services.calls("banChatMember").is_empty());
}

#[tokio::test]
async fn first_seen_markers_survive_join_leave_and_mode_changes() {
    let (e, _) = setup(Some(0.1));
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.message(CHAT, &message(2040)).unwrap();
    drain(&e).await;
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(2041)).unwrap();
    e.message(CHAT, &json!({"date":NOW,"left_chat_member":{"id":USER}}))
        .unwrap();
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(2042)).unwrap();
    select_mode(&e, CHAT, ScreeningMode::NewMembers);
    e.message(CHAT, &message(2043)).unwrap();
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.message(CHAT, &message(2044)).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn switching_modes_or_enabling_spam_does_not_recheck_recorded_speakers() {
    let (e, _) = setup(Some(0.99));
    // Default new-member mode does not screen an unobserved member, but remembers them.
    e.message(CHAT, &message(2050)).unwrap();
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.message(CHAT, &message(2051)).unwrap();
    drain(&e).await;
    assert!(cases(&e).is_empty());

    let (e, _) = setup(Some(0.99));
    e.store
        .put(
            CHAT,
            "settings",
            &Settings {
                spam: false,
                screening: ScreeningMode::FirstSeen,
                ..Settings::default()
            },
        )
        .unwrap();
    e.message(CHAT, &message(2052)).unwrap();
    let mut settings = e.settings(CHAT).unwrap();
    settings.spam = true;
    e.store.put(CHAT, "settings", &settings).unwrap();
    e.message(CHAT, &message(2053)).unwrap();
    drain(&e).await;
    assert!(cases(&e).is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn first_seen_cases_keep_their_mode_and_cannot_be_evaded_by_joining() {
    for rejoin in [false, true] {
        let (e, _) = setup(Some(0.7));
        select_mode(&e, CHAT, ScreeningMode::FirstSeen);
        e.message(CHAT, &message(2060)).unwrap();
        drain(&e).await;
        let old = cases(&e)[0].clone();
        select_mode(&e, CHAT, ScreeningMode::NewMembers);
        if rejoin {
            e.joined(CHAT, USER, NOW, true).unwrap();
        }
        assert!(admin::decide(&e, CHAT, &old.id, true, 7).unwrap());
        drain(&e).await;
        assert_eq!(cases(&e)[0].state, "banned");
        assert_eq!(e.services.calls("banChatMember")[0]["user_id"], USER);
        assert_eq!(e.services.calls("deleteMessage")[0]["message_id"], 2060);
    }
}

#[tokio::test]
async fn first_seen_is_independent_of_known_membership_dates() {
    let (e, time) = setup(Some(0.1));
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.joined(CHAT, USER, NOW + 1, true).unwrap();
    e.message(CHAT, &message(2061)).unwrap();
    time.fetch_add(1, Ordering::SeqCst);
    let mut current = message(2062);
    current["date"] = json!(e.now());
    e.message(CHAT, &current).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].message, 2061);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn first_seen_ignores_channel_posts_anonymous_services_and_unmanaged_groups() {
    let (e, _) = setup(Some(0.99));
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    for (i, patch) in [
        json!({"is_automatic_forward":true}),
        json!({"sender_chat":{"id":CHAT}}),
        json!({"text":null,"pinned_message":{"message_id":1}}),
        json!({"from":{"id":0}}),
        json!({"from":{"id":999,"is_bot":true}}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut m = message(2070 + i as i64);
        m.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        e.message(CHAT, &m).unwrap();
    }
    e.ingest(&json!({"update_id":2080,"edited_message":message(2080)}))
        .unwrap();
    e.message(CHAT - 1, &message(2081)).unwrap();
    drain(&e).await;
    assert!(
        e.store
            .list::<FirstMessage>(CHAT, "first_message:", None, 0, 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        e.store
            .list::<FirstMessage>(CHAT - 1, "first_message:", None, 0, 100)
            .unwrap()
            .is_empty()
    );
    assert!(cases(&e).is_empty());
    e.message(CHAT, &message(2082)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].message, 2082);
}

#[tokio::test]
async fn first_seen_persists_after_restart_and_case_retention() {
    let (mut e, time) = setup(Some(0.1));
    let dir = std::env::temp_dir().join(format!("fuuki-iin-first-seen-{}", uuid::Uuid::new_v4()));
    let path = dir.join("bot.sqlite");
    e.store = Arc::new(Store::open(&path).unwrap());
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.message(CHAT, &message(2090)).unwrap();
    drain(&e).await;
    e.store = Arc::new(Store::memory().unwrap()); // Close every connection to the file.
    e.store = Arc::new(Store::open(&path).unwrap());
    assert_eq!(
        e.settings(CHAT).unwrap().screening,
        ScreeningMode::FirstSeen
    );
    time.fetch_add(40 * 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert!(cases(&e).is_empty());
    let mut next = message(2091);
    next["date"] = json!(e.now());
    e.message(CHAT, &next).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn legacy_settings_and_first_message_records_migrate_without_rescreening() {
    let (mut e, _) = setup(Some(0.99));
    let dir = std::env::temp_dir().join(format!("fuuki-iin-legacy-{}", uuid::Uuid::new_v4()));
    let path = dir.join("bot.sqlite");
    {
        let store = Store::open(&path).unwrap();
        store
            .put(
                CHAT,
                "settings",
                &json!({"spam":true,"captcha":false,"review":0.6,"ban":0.9}),
            )
            .unwrap();
        store.put(CHAT, &format!("member:{USER}"), &json!({"user":USER,"cycle":"legacy","joined_at":NOW-10,"active":true,"first_message":1,"provisional":false})).unwrap();
    }
    e.store = Arc::new(Store::open(&path).unwrap());
    assert_eq!(
        e.settings(CHAT).unwrap().screening,
        ScreeningMode::NewMembers
    );
    assert!(!e.settings(CHAT).unwrap().captcha);
    let marker = e
        .store
        .get::<FirstMessage>(CHAT, &format!("first_message:{USER}"))
        .unwrap()
        .unwrap();
    assert_eq!(marker.message, 1);
    select_mode(&e, CHAT, ScreeningMode::FirstSeen);
    e.message(CHAT, &message(2100)).unwrap();
    drain(&e).await;
    assert!(e.services.evidence.lock().unwrap().is_empty());
    // Old queued cases have no screening field and retain their original semantics.
    let mut legacy = json!({"id":"old","cycle":"legacy","user":USER,"message":1,"joined_at":NOW,"created_at":NOW,"evidence":null,"state":"allowed","policy":"spam-v1","review_threshold":0.6,"ban_threshold":0.9});
    assert_eq!(
        serde_json::from_value::<Case>(legacy.clone())
            .unwrap()
            .screening,
        ScreeningMode::NewMembers
    );
    legacy["screening"] = json!("unknown");
    assert!(serde_json::from_value::<Case>(legacy).is_err());
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn mode_buttons_require_current_write_permissions_and_remain_private() {
    let (mut e, _) = setup(None);
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![8];
    e.services.member(
        9,
        json!({"status":"administrator","can_delete_messages":true,"can_restrict_members":true}),
    );
    for user in [USER, 8, 9] {
        admin::handle(&e, CHAT, &admin_callback(user, CHAT, "firstseen", ""))
            .await
            .unwrap();
        assert_eq!(
            e.settings(CHAT).unwrap().screening,
            ScreeningMode::NewMembers
        );
    }
    admin::handle(&e, CHAT, &admin_message(8, &format!("/admin {CHAT}")))
        .await
        .unwrap();
    let view = e.services.calls("sendMessage");
    assert!(
        !view.last().unwrap()["reply_markup"]
            .to_string()
            .contains("firstseen|")
    );
    e.services.member(7, json!({"status":"creator"}));
    for _ in 0..2 {
        admin::handle(&e, CHAT, &admin_callback(7, CHAT, "firstseen", ""))
            .await
            .unwrap();
        assert_eq!(
            e.settings(CHAT).unwrap().screening,
            ScreeningMode::FirstSeen
        );
        assert!(e.settings(CHAT).unwrap().captcha);
    }
    assert!(
        e.store
            .audits(CHAT)
            .unwrap()
            .iter()
            .any(|a| a["action"] == "settings_changed" && a["actor"] == 7)
    );
    e.services.member(7, json!({"status":"member"}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "newmembers", ""))
        .await
        .unwrap();
    assert_eq!(
        e.settings(CHAT).unwrap().screening,
        ScreeningMode::FirstSeen
    );
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "newmembers", ""))
        .await
        .unwrap();
    assert_eq!(
        e.settings(CHAT).unwrap().screening,
        ScreeningMode::NewMembers
    );
    assert!(
        e.services
            .calls("sendMessage")
            .iter()
            .all(|r| r["chat_id"].as_i64().unwrap() > 0)
    );
    assert_chinese_admin_replies(&e);
}
