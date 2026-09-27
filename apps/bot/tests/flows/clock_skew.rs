use super::*;
use fuuki_iin_bot::model::{FirstMessage, ScreeningMode};

fn first_seen(e: &Engine<Mock>) {
    let mut settings = e.settings(CHAT).unwrap();
    settings.screening = ScreeningMode::FirstSeen;
    e.store.put(CHAT, "settings", &settings).unwrap();
}

#[tokio::test]
async fn future_first_seen_update_is_screened_before_the_update_is_completed() {
    let (mut e, _) = setup(Some(0.96));
    first_seen(&e);
    e.services.member(USER, json!({"status":"left"}));
    let output = support::Capture::default();
    e.logger = Arc::new(Logger::with_writer(LogLevel::Info, &[], output.clone()));
    let mut m = message(1);
    m["date"] = json!(NOW + 299);
    m["text"] = json!("Synthetic promotion https://example.invalid/offer");
    let update = json!({"update_id":1,"message":m});
    e.queue(0, "update", "1", update.clone()).unwrap();
    let job = e.store.claim(NOW).unwrap().unwrap();
    e.run(&job).await.unwrap();
    assert_eq!(e.store.stats(0).unwrap()["done"], 1);
    // A completed update must already have its durable case and classify job.
    assert_eq!(cases(&e)[0].state, "classifying");
    assert_eq!(e.store.stats(CHAT).unwrap()["pending"], 1);
    assert_eq!(
        e.store
            .get::<FirstMessage>(CHAT, &format!("first_message:{USER}"))
            .unwrap()
            .unwrap()
            .message,
        1
    );
    drain(&e).await;
    e.queue(0, "update", "1", update).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(cases(&e)[0].state, "banned");
    assert_eq!(e.services.calls("deleteMessage")[0]["message_id"], 1);
    assert_eq!(e.services.calls("banChatMember")[0]["user_id"], USER);
    let records = output.records();
    let skew = records
        .iter()
        .find(|r| r["event"] == "telegram.clock_skew")
        .unwrap();
    assert_eq!(skew["fields"]["ahead_seconds"], 299);
    assert_eq!(skew["fields"]["kind"], "message");
    assert_eq!(skew["fields"]["local_date"], NOW);
    assert_eq!(skew["fields"]["telegram_date"], NOW + 299);
    assert!(skew["body"].is_null());
    assert!(!output.text().contains("Synthetic promotion"));
    assert!(!records.iter().any(|r| r["event"] == "screening.skip"));
}

#[tokio::test]
async fn future_membership_updates_still_establish_the_first_message_cycle() {
    for service_message in [false, true] {
        let (e, _) = setup(Some(0.96));
        let joined = NOW + 299;
        let update = if service_message {
            json!({"update_id":1,"message":{"message_id":1,"date":joined,"chat":{"id":CHAT},"new_chat_members":[{"id":USER}]}})
        } else {
            json!({"update_id":1,"chat_member":{"date":joined,"chat":{"id":CHAT},"old_chat_member":{"status":"left","user":{"id":USER}},"new_chat_member":{"status":"member","user":{"id":USER}}}})
        };
        e.ingest(&update).unwrap();
        let mut m = message(2);
        m["date"] = json!(joined);
        e.ingest(&json!({"update_id":2,"message":m})).unwrap();
        drain(&e).await;
        let member = e
            .store
            .get::<Member>(CHAT, &format!("member:{USER}"))
            .unwrap()
            .unwrap();
        assert_eq!(member.joined_at, joined);
        assert_eq!(member.first_message, Some(2));
        assert_eq!(cases(&e)[0].state, "banned");
        assert_eq!(cases(&e)[0].joined_at, joined);
        assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn stale_messages_do_not_consume_first_seen_and_skips_explain_scope_without_content() {
    let (mut e, _) = setup(Some(0.07));
    first_seen(&e);
    let output = support::Capture::default();
    e.logger = Arc::new(Logger::with_writer(LogLevel::Verbose, &[], output.clone()));
    for (i, date) in [0, -1, NOW - 604800, NOW - 604801].into_iter().enumerate() {
        let mut m = message(i as i64 + 1);
        m["date"] = json!(date);
        e.message(CHAT, &m).unwrap();
    }
    assert!(cases(&e).is_empty());
    assert!(
        e.store
            .get::<FirstMessage>(CHAT, &format!("first_message:{USER}"))
            .unwrap()
            .is_none()
    );
    e.message(CHAT, &message(5)).unwrap();
    drain(&e).await;
    let mut next = message(6);
    next["text"] = json!("private-second-message-body");
    e.message(CHAT, &next).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].message, 5);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    let skips: Vec<_> = output
        .records()
        .into_iter()
        .filter(|r| r["event"] == "screening.skip")
        .collect();
    assert_eq!(skips.len(), 5);
    assert!(
        skips[..4]
            .iter()
            .all(|r| r["fields"]["reason"] == "invalid_or_stale_date")
    );
    assert_eq!(skips[4]["fields"]["reason"], "not_first_message");
    assert_eq!(skips[4]["fields"]["message_id"], 6);
    assert_eq!(skips[4]["fields"]["user_id"], USER);
    assert!(skips.iter().all(|r| r["body"].is_null()));
    assert!(!output.text().contains("private-second-message-body"));
}

#[tokio::test]
async fn future_join_request_is_retried_visibly_without_issuing_early_credentials() {
    let (mut e, time) = setup(Some(0.96));
    let output = support::Capture::default();
    e.logger = Arc::new(Logger::with_writer(LogLevel::Info, &[], output.clone()));
    e.queue(
        0,
        "update",
        "1",
        json!({"update_id":1,"chat_join_request":join(NOW + 299)}),
    )
    .unwrap();
    drain(&e).await;
    assert_eq!(e.store.stats(0).unwrap()["pending"], 1);
    assert!(e.store.stats(0).unwrap()["done"].is_null());
    assert!(
        e.store
            .get::<Value>(CHAT, &format!("latest:{USER}"))
            .unwrap()
            .is_none()
    );
    assert!(e.services.calls("sendMessage").is_empty());
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
    let records = output.records();
    let error = records.iter().find(|r| r["event"] == "job.error").unwrap();
    assert_eq!(error["fields"]["error"], "join_request_clock_skew");
    assert_eq!(error["fields"]["exhausted"], false);
    // The durable update can resume normally after clock synchronisation.
    time.store(NOW + 299, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(e.store.stats(0).unwrap()["done"], 1);
    assert_eq!(session(&e).state, "pending");
    assert_eq!(session(&e).claims.iat, NOW + 299);
    assert_eq!(e.services.calls("sendMessage").len(), 1);
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn accepting_future_moderation_updates_does_not_relax_captcha_freshness() {
    let (e, _) = setup(Some(0.96));
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    drain(&e).await;
    for (i, date) in [NOW + 31, NOW + 299, NOW - 300].into_iter().enumerate() {
        let mut m = submission(&e, &session(&e), USER);
        m["date"] = json!(date);
        m["message_id"] = json!(20 + i);
        e.ingest(&json!({"update_id":20 + i,"message":m})).unwrap();
        drain(&e).await;
        assert_eq!(session(&e).state, "pending");
        assert!(session(&e).verification.is_none());
        assert!(e.services.calls("siteverify").is_empty());
        assert!(e.services.calls("approveChatJoinRequest").is_empty());
    }
    assert!(e.services.evidence.lock().unwrap().is_empty());
}
