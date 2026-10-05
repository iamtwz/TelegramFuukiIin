use super::*;
use fuuki_iin_bot::model::ScreeningMode;

fn first_seen(e: &Engine<Mock>) {
    let mut settings = e.settings(CHAT).unwrap();
    settings.screening = ScreeningMode::FirstSeen;
    e.store.put(CHAT, "settings", &settings).unwrap();
    e.services.member(USER, json!({"status":"left"}));
}

async fn enforce_once(e: &Engine<Mock>) {
    for _ in 0..20 {
        if cases(e)[0].banned {
            // Mock the membership returned after a successful Telegram ban.
            e.services.member(USER, json!({"status":"kicked"}));
            return;
        }
        let job = e.store.claim(e.now()).unwrap().unwrap();
        e.run(&job).await.unwrap();
    }
    panic!("enforcement did not ban the target");
}

fn followups(e: &Engine<Mock>, id: &str) -> Vec<Value> {
    e.store
        .list(CHAT, &format!("case_message:{id}:"), None, 0, 1000)
        .unwrap()
}

#[tokio::test]
async fn review_ban_deletes_same_senders_posts_across_topics_without_reclassifying() {
    let (e, _) = setup(Some(0.7));
    first_seen(&e);
    e.services.member(7, json!({"status":"creator"}));
    let mut first = message(3100);
    first["message_thread_id"] = json!(100);
    e.message(CHAT, &first).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    for (message_id, topic, content) in [(3101, 200, "Hello again"), (3102, 300, "Another post")] {
        let mut post = message(message_id);
        post["message_thread_id"] = json!(topic);
        post["text"] = json!(content);
        e.ingest(&json!({"update_id":message_id,"message":post}))
            .unwrap();
        e.message(CHAT, &post).unwrap();
    }
    assert_eq!(followups(&e, &id).len(), 2);
    assert!(e.services.calls("deleteMessage").is_empty());
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "ban", &id))
        .await
        .unwrap();
    enforce_once(&e).await;
    drain(&e).await;
    let deletes = e.services.calls("deleteMessage");
    assert_eq!(
        deletes
            .iter()
            .map(|v| v["message_id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![3100, 3101, 3102]
    );
    assert!(deletes.iter().all(|v| v["chat_id"] == CHAT));
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert!(
        followups(&e, &id)
            .iter()
            .all(|v| v["deleted"] == true && v.get("text").is_none())
    );
    let audit = e.store.audits(CHAT).unwrap();
    for message_id in [3100, 3101, 3102] {
        assert!(audit.iter().any(|a| {
            a["action"] == "message_deleted"
                && a["target"] == id
                && a["detail"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("message_id={message_id}"))
        }));
    }
}

#[tokio::test]
async fn automatic_ban_cleans_media_observed_while_classifying() {
    let (e, _) = setup(Some(0.99));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(3200)).unwrap();
    for (id, patch) in [
        (
            3201,
            json!({"text":null,"photo":[{"file_id":"synthetic-photo"}]}),
        ),
        (
            3202,
            json!({"text":null,"sticker":{"emoji":"🐱","file_id":"synthetic-sticker"}}),
        ),
    ] {
        let mut m = message(id);
        m.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        e.message(CHAT, &m).unwrap();
    }
    enforce_once(&e).await;
    drain(&e).await;
    assert_eq!(e.services.calls("deleteMessage").len(), 3);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn allowed_cases_never_clean_followups_or_track_later_posts() {
    let (e, _) = setup(Some(0.1));
    first_seen(&e);
    e.message(CHAT, &message(3300)).unwrap();
    e.message(CHAT, &message(3301)).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    e.message(CHAT, &message(3302)).unwrap();
    assert_eq!(followups(&e, &id).len(), 1);
    assert!(e.services.calls("deleteMessage").is_empty());
    assert!(e.services.calls("banChatMember").is_empty());

    let (e, _) = setup(Some(0.7));
    first_seen(&e);
    e.message(CHAT, &message(3303)).unwrap();
    e.message(CHAT, &message(3304)).unwrap();
    drain(&e).await;
    assert!(admin::decide(&e, CHAT, &cases(&e)[0].id, false, 7).unwrap());
    drain(&e).await;
    assert!(e.services.calls("deleteMessage").is_empty());
}

#[tokio::test]
async fn cleanup_retries_only_failed_post_after_database_reopen() {
    let (mut e, time) = setup(Some(0.99));
    let dir = std::env::temp_dir().join(format!("fuuki-cleanup-{}", uuid::Uuid::new_v4()));
    let path = dir.join("bot.sqlite");
    e.store = Arc::new(Store::open(&path).unwrap());
    first_seen(&e);
    for id in [3400, 3401, 3402] {
        e.message(CHAT, &message(id)).unwrap();
    }
    enforce_once(&e).await;
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("deleteMessage".into(), 1);
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    assert_eq!(
        followups(&e, &id)
            .iter()
            .filter(|m| m["deleted"] == true)
            .count(),
        1
    );
    e.store = Arc::new(Store::memory().unwrap());
    e.store = Arc::new(Store::open(&path).unwrap());
    time.store(NOW + 10, Ordering::SeqCst);
    e.store.recover(e.now()).unwrap();
    drain(&e).await;
    for message_id in [3400, 3401, 3402] {
        assert_eq!(
            e.services
                .calls("deleteMessage")
                .iter()
                .filter(|m| m["message_id"] == message_id)
                .count(),
            if message_id == 3401 { 2 } else { 1 }
        );
    }
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert!(followups(&e, &id).iter().all(|m| m["deleted"] == true));
    e.message(CHAT, &message(3401)).unwrap();
    drain(&e).await;
    assert_eq!(e.services.calls("deleteMessage").len(), 4);
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn delayed_updates_are_cleaned_only_if_sent_before_ban_including_clock_lag() {
    for clock_lag in [0, 120] {
        let (e, time) = setup(Some(0.99));
        first_seen(&e);
        let mut first = message(3500);
        first["date"] = json!(NOW + clock_lag);
        e.message(CHAT, &first).unwrap();
        time.store(NOW + 10, Ordering::SeqCst);
        enforce_once(&e).await;
        drain(&e).await;
        for (id, at) in [
            (3501, NOW + clock_lag + 9),
            (3502, NOW + clock_lag + 11),
            (3503, NOW + clock_lag - 1),
        ] {
            let mut delayed = message(id);
            delayed["date"] = json!(at);
            e.message(CHAT, &delayed).unwrap();
            e.message(CHAT, &delayed).unwrap();
        }
        drain(&e).await;
        assert_eq!(
            e.services
                .calls("deleteMessage")
                .iter()
                .map(|m| m["message_id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![3500, 3501]
        );
        assert_eq!(e.services.calls("banChatMember").len(), 1);
        assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn queued_cleanup_respects_promotions_unbans_new_cycles_and_removed_groups() {
    for action in ["promote", "unban", "rejoin", "unmanaged", "cancel"] {
        let (mut e, _) = setup(Some(0.99));
        e.joined(CHAT, USER, NOW, true).unwrap();
        e.message(CHAT, &message(3600)).unwrap();
        e.message(CHAT, &message(3601)).unwrap();
        enforce_once(&e).await;
        match action {
            "promote" => e.services.member(USER, json!({"status":"administrator"})),
            "unban" => e.services.member(USER, json!({"status":"member"})),
            "rejoin" => {
                e.joined(CHAT, USER, NOW + 1, true).unwrap();
            }
            "unmanaged" => Arc::get_mut(&mut e.config).unwrap().chats.clear(),
            "cancel" => {
                let mut c = cases(&e)[0].clone();
                c.state = "allowed".into();
                e.store.put(CHAT, &format!("case:{}", c.id), &c).unwrap();
            }
            _ => unreachable!(),
        }
        drain(&e).await;
        assert_eq!(e.services.calls("deleteMessage").len(), 1, "{action}");
        assert_eq!(e.services.calls("banChatMember").len(), 1, "{action}");
    }
}

#[tokio::test]
async fn cleanup_metadata_is_retained_and_pruned_with_its_case() {
    let (e, time) = setup(Some(0.1));
    first_seen(&e);
    e.message(CHAT, &message(3700)).unwrap();
    e.message(CHAT, &message(3701)).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    time.store(NOW + 8 * 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert_eq!(followups(&e, &id).len(), 1);
    time.store(NOW + 38 * 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert!(followups(&e, &id).is_empty());
    assert!(
        e.store
            .get::<Value>(CHAT, &format!("message_case:{USER}"))
            .unwrap()
            .is_none()
    );
    assert!(
        e.store
            .get::<Value>(CHAT, &format!("first_message:{USER}"))
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn followups_are_bound_to_real_sender_group_and_supported_messages() {
    let (mut e, _) = setup(Some(0.99));
    Arc::get_mut(&mut e.config).unwrap().chats.push(CHAT - 1);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(3800)).unwrap();
    let id = cases(&e)[0].id.clone();
    for (message_id, patch) in [
        (3801, json!({"from":{"id":USER+1}})),
        (3802, json!({"sender_chat":{"id":CHAT}})),
        (3803, json!({"is_automatic_forward":true})),
        (3804, json!({"date":NOW-1})),
        (
            3805,
            json!({"text":null,"pinned_message":{"message_id":100}}),
        ),
        (3806, json!({"business_connection_id":"synthetic"})),
    ] {
        let mut m = message(message_id);
        m.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        e.message(CHAT, &m).unwrap();
    }
    e.message(CHAT - 1, &message(3807)).unwrap();
    e.message(CHAT, &message(3799)).unwrap();
    assert!(followups(&e, &id).is_empty());
    enforce_once(&e).await;
    drain(&e).await;
    assert_eq!(e.services.calls("deleteMessage").len(), 1);
}

#[tokio::test]
async fn cleanup_schedules_every_post_across_storage_pages() {
    let (e, _) = setup(Some(0.99));
    first_seen(&e);
    for id in 3900..4003 {
        e.message(CHAT, &message(id)).unwrap();
    }
    enforce_once(&e).await;
    for _ in 0..120 {
        let Some(job) = e.store.claim(e.now()).unwrap() else {
            break;
        };
        e.run(&job).await.unwrap();
    }
    assert_eq!(e.services.calls("deleteMessage").len(), 103);
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert_eq!(
        e.store.stats(CHAT).unwrap()["pending"]
            .as_i64()
            .unwrap_or(0),
        0
    );
}

#[tokio::test]
async fn exhausted_cleanup_alerts_privately_and_manual_retry_does_not_reban() {
    let (e, time) = setup(Some(0.99));
    first_seen(&e);
    e.services.member(7, json!({"status":"creator"}));
    e.message(CHAT, &message(4100)).unwrap();
    e.message(CHAT, &message(4101)).unwrap();
    enforce_once(&e).await;
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("deleteMessage".into(), 6);
    for offset in [0, 10, 30, 60, 120, 240] {
        time.store(NOW + offset, Ordering::SeqCst);
        drain(&e).await;
    }
    assert_eq!(e.store.stats(CHAT).unwrap()["dead"], 1);
    assert!(
        e.store.audits(CHAT).unwrap().iter().any(
            |a| a["action"] == "job_failed" && a["target"].as_str().unwrap().ends_with(":4101")
        )
    );
    assert!(
        e.services
            .calls("sendMessage")
            .iter()
            .any(|m| m["chat_id"] == 7 && m["text"].as_str().unwrap().contains("有任务失败"))
    );
    assert!(
        e.services
            .calls("sendMessage")
            .iter()
            .all(|m| m["chat_id"] != CHAT)
    );
    assert_eq!(e.store.retry(CHAT, e.now()).unwrap(), 1);
    drain(&e).await;
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert!(
        followups(&e, &cases(&e)[0].id)
            .iter()
            .all(|m| m["deleted"] == true)
    );
}
