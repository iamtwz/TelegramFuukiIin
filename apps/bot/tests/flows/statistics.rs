use super::*;
use fuuki_iin_bot::statistics::{RETENTION_DAYS, Report, day};

fn report(e: &Engine<Mock>, at: i64) -> Report {
    e.store
        .statistics_report(CHAT, day(at), day(at), e.now())
        .unwrap()
}
fn all_text(e: &Engine<Mock>) -> String {
    e.services
        .replies()
        .iter()
        .map(super::formatting::rendered)
        .collect::<Vec<_>>()
        .join("\n")
}
fn usage() -> Usage {
    Usage {
        input_tokens: Some(100),
        output_tokens: Some(4),
        cost_usd: Some(0.0000042),
    }
}

#[tokio::test]
async fn token_usage_uses_call_day_while_detection_uses_case_day() {
    let (e, time) = setup(None);
    let midnight = (day(NOW) + 1) * 86400 - 28800;
    time.store(midnight - 1, Ordering::SeqCst);
    *e.services.usage.lock().unwrap() = usage();
    e.joined(CHAT, USER, e.now(), true).unwrap();
    let mut m = message(1);
    m["date"] = json!(e.now());
    e.message(CHAT, &m).unwrap();
    drain(&e).await;
    time.store(midnight + 10, Ordering::SeqCst);
    *e.services.probability.lock().unwrap() = Some(0.95);
    drain(&e).await;
    let before = report(&e, midnight - 1);
    let after = report(&e, midnight);
    assert_eq!(
        (before.messages, before.spam, before.attempts, before.input),
        (1, 1, 1, 100)
    );
    assert_eq!(
        (after.messages, after.spam, after.attempts, after.input),
        (0, 0, 1, 100)
    );
}

#[test]
fn legacy_schema_migrates_without_fabricating_historical_statistics() {
    let dir = std::env::temp_dir().join(format!(
        "fuuki-iin-statistics-legacy-{}",
        uuid::Uuid::new_v4()
    ));
    let path = dir.join("bot.sqlite");
    drop(Store::open(&path).unwrap());
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("DROP TABLE statistics_cases; DROP TABLE statistics_joins; DROP TABLE statistics_usage; DROP TABLE statistics_coverage; PRAGMA user_version=1;").unwrap();
        db.execute("INSERT INTO audit(chat,at,action,target,detail) VALUES (?1,?2,'classified','old-case','p=0.9')",rusqlite::params![CHAT,NOW-86400]).unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        let old = store
            .statistics_report(CHAT, day(NOW) - 1, day(NOW) - 1, NOW)
            .unwrap();
        assert!(old.unavailable);
        assert!(old.partial);
        assert_eq!(old.attempts, 0);
        assert_eq!(store.audits(CHAT).unwrap().len(), 1);
    }
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
    drop(Store::open(&path).unwrap());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn unique_applicants_and_cross_midnight_approvals_use_application_cohort() {
    let (e, time) = setup(None);
    let midnight = (day(NOW) + 1) * 86400 - 28800;
    time.store(midnight - 60, Ordering::SeqCst);
    e.store.start_statistics(CHAT, e.now()).unwrap();
    let request = join(e.now());
    e.join_request(CHAT, &request, 10).unwrap();
    e.join_request(CHAT, &request, 10).unwrap();
    e.join_request(CHAT, &request, 11).unwrap();
    let mut other = request.clone();
    other["from"]["id"] = json!(43);
    other["user_chat_id"] = json!(43);
    e.join_request(CHAT, &other, 12).unwrap();
    assert_eq!(report(&e, e.now()).applicants, 2);
    drain(&e).await;
    time.store(midnight + 10, Ordering::SeqCst);
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    drain(&e).await;
    let s = session(&e);
    e.queue(
        CHAT,
        "approve",
        "duplicate-approve",
        json!({"id":s.claims.nonce}),
    )
    .unwrap();
    drain(&e).await;
    let yesterday = report(&e, midnight - 1);
    assert_eq!((yesterday.applicants, yesterday.approved), (2, 1));
    let today = report(&e, e.now());
    assert_eq!((today.applicants, today.approved), (0, 0));
    e.join_request(CHAT, &join(e.now()), 13).unwrap();
    assert_eq!(report(&e, e.now()).applicants, 1);
    let sum = e
        .store
        .statistics_report(CHAT, day(midnight - 1), day(e.now()), e.now())
        .unwrap();
    assert_eq!((sum.applicants, sum.approved), (3, 1));
}

#[tokio::test]
async fn requests_count_with_verification_disabled_but_not_in_unmanaged_groups() {
    let (e, _) = setup(None);
    let mut settings = e.settings(CHAT).unwrap();
    settings.captcha = false;
    e.store.put(CHAT, "settings", &settings).unwrap();
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    e.join_request(CHAT, &join(NOW), 2).unwrap();
    e.join_request(CHAT - 1, &join(NOW), 3).unwrap();
    drain(&e).await;
    assert_eq!(report(&e, NOW).applicants, 1);
    assert!(e.services.calls("sendMessage").is_empty());
    assert_eq!(
        e.store
            .statistics_report(CHAT - 1, day(NOW), day(NOW), NOW)
            .unwrap()
            .applicants,
        0
    );
}

#[tokio::test]
async fn detection_and_spam_counts_deduplicate_and_keep_case_thresholds() {
    let (e, _) = setup(Some(0.8));
    *e.services.usage.lock().unwrap() = usage();
    let mut settings = e.settings(CHAT).unwrap();
    settings.screening = fuuki_iin_bot::model::ScreeningMode::FirstSeen;
    e.store.put(CHAT, "settings", &settings).unwrap();
    for (user, p) in [(42, 0.8), (43, 0.6), (44, 0.59)] {
        *e.services.probability.lock().unwrap() = Some(p);
        let mut m = message(user);
        m["from"]["id"] = json!(user);
        e.message(CHAT, &m).unwrap();
        e.message(CHAT, &m).unwrap();
        drain(&e).await;
    }
    let before = report(&e, NOW);
    assert_eq!(
        (
            before.messages,
            before.classified,
            before.spam,
            before.review,
            before.attempts
        ),
        (3, 3, 1, 1, 3)
    );
    assert_eq!((before.input, before.output), (300, 12));
    assert!((before.cost - 0.0000126).abs() < 1e-12);
    settings.ban = 0.95;
    e.store.put(CHAT, "settings", &settings).unwrap();
    let review = cases(&e)
        .into_iter()
        .find(|c| c.probability == Some(0.6))
        .unwrap();
    assert!(admin::decide(&e, CHAT, &review.id, false, 7).unwrap());
    drain(&e).await;
    let after = report(&e, NOW);
    assert_eq!((after.messages, after.spam, after.review), (3, 1, 1));
    e.services.member(45, json!({"status":"creator"}));
    let mut protected = message(45);
    protected["from"]["id"] = json!(45);
    e.message(CHAT, &protected).unwrap();
    drain(&e).await;
    assert_eq!((report(&e, NOW).skipped, report(&e, NOW).attempts), (1, 3));
}

#[tokio::test]
async fn retries_count_usage_per_attempt_without_counting_messages_twice() {
    let (e, time) = setup(None);
    *e.services.usage.lock().unwrap() = usage();
    *e.services.classify_transport_failures.lock().unwrap() = 1;
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await; // Transport error: unknown, never zero.
    time.fetch_add(10, Ordering::SeqCst);
    drain(&e).await; // Invalid decision with known usage.
    *e.services.probability.lock().unwrap() = Some(0.1);
    time.fetch_add(10, Ordering::SeqCst);
    drain(&e).await;
    let r = report(&e, NOW);
    assert_eq!(
        (
            r.messages,
            r.classified,
            r.attempts,
            r.input_known,
            r.output_known
        ),
        (1, 1, 3, 2, 2)
    );
    assert_eq!((r.input, r.output), (200, 8));
    assert!((r.cost - 0.0000084).abs() < 1e-12);
    let id = cases(&e)[0].id.clone();
    e.queue(CHAT, "classify", "duplicate-result", json!({"id":id}))
        .unwrap();
    drain(&e).await;
    assert_eq!(report(&e, NOW).attempts, 3);
}

#[tokio::test]
async fn exhausted_classification_is_failure_not_clean_or_spam() {
    let (e, time) = setup(None);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    for _ in 0..3 {
        drain(&e).await;
        time.fetch_add(60, Ordering::SeqCst);
    }
    let r = report(&e, NOW);
    assert_eq!(
        (
            r.messages,
            r.failed,
            r.classified,
            r.spam,
            r.pending,
            r.attempts
        ),
        (1, 1, 0, 0, 0, 3)
    );
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, CHAT, &admin_message(7, &format!("/stats {CHAT}")))
        .await
        .unwrap();
    let text = all_text(&e);
    assert!(text.contains("Spam 率：—"));
    assert!(text.contains("Tokens 合计：未知"));
    assert!(!text.contains("Tokens 合计：0"));
}

#[tokio::test]
async fn statistics_survive_record_cleanup_restart_and_expire_after_a_year() {
    let (mut e, time) = setup(Some(0.799999));
    let dir = std::env::temp_dir().join(format!("fuuki-iin-statistics-{}", uuid::Uuid::new_v4()));
    let path = dir.join("bot.sqlite");
    e.store = Arc::new(Store::open(&path).unwrap());
    *e.services.usage.lock().unwrap() = usage();
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    drain(&e).await;
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    drain(&e).await;
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    time.store(NOW + 40 * 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert!(cases(&e).is_empty());
    assert!(e.store.audits(CHAT).unwrap().is_empty());
    e.store = Arc::new(Store::memory().unwrap());
    e.store = Arc::new(Store::open(&path).unwrap());
    let r = report(&e, NOW);
    assert_eq!(
        (
            r.applicants,
            r.approved,
            r.messages,
            r.review,
            r.input,
            r.output
        ),
        (1, 1, 1, 1, 100, 4)
    );
    time.store(NOW + RETENTION_DAYS * 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    let r = report(&e, NOW);
    assert_eq!((r.applicants, r.messages, r.attempts), (0, 0, 0));
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn usage_intent_survives_recovery_and_repeated_result_writes_are_idempotent() {
    let (e, _) = setup(None);
    let id = e.store.start_usage(CHAT, NOW).unwrap();
    e.store.recover(NOW).unwrap();
    assert_eq!(
        (report(&e, NOW).attempts, report(&e, NOW).input_known),
        (1, 0)
    );
    e.store.finish_usage(CHAT - 1, &id, &usage()).unwrap();
    assert_eq!(report(&e, NOW).input_known, 0);
    e.store.finish_usage(CHAT, &id, &usage()).unwrap();
    e.store.finish_usage(CHAT, &id, &usage()).unwrap();
    assert_eq!((report(&e, NOW).attempts, report(&e, NOW).input), (1, 100));
}

#[tokio::test]
async fn statistics_callbacks_check_current_authority_scope_and_dates() {
    let (mut e, _) = setup(None);
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![7];
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    for (n, action) in ["stats", "stats"].iter().enumerate() {
        let mut update = admin_callback(7, CHAT, action, "");
        update["update_id"] = json!(9000 + n);
        e.ingest(&update).unwrap();
    }
    drain(&e).await;
    assert!(all_text(&e).contains("申请人数：1"));
    e.services.calls.lock().unwrap().clear();
    for raw in [
        format!("w:{}", day(NOW)),
        format!("m:{}", day(NOW)),
        report(&e, NOW).start,
        String::new(),
        format!("d:{}", day(NOW)),
    ] {
        admin::handle(&e, CHAT, &admin_callback(7, CHAT, "stats", &raw))
            .await
            .unwrap();
    }
    assert!(all_text(&e).contains("群统计"));
    for raw in [
        "2026-02-30",
        "9999-12-31",
        "2000-01-01",
        "d:-9223372036854775808",
        "m:9223372036854775807",
        "d:1:2",
        "x:20721",
        "not-a-date",
    ] {
        admin::handle(&e, CHAT, &admin_callback(7, CHAT, "stats", raw))
            .await
            .unwrap();
        assert!(formatting::rendered(e.services.replies().last().unwrap()).contains("日期无效"));
    }
    e.services.calls.lock().unwrap().clear();
    admin::handle(&e, CHAT, &admin_callback(8, CHAT, "stats", ""))
        .await
        .unwrap();
    assert!(e.services.calls("sendMessage").is_empty());
    Arc::get_mut(&mut e.config).unwrap().chats.clear();
    e.services.calls.lock().unwrap().clear();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "stats", ""))
        .await
        .unwrap();
    assert!(e.services.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stats_menu_is_private_and_daily_navigation_has_bounded_callbacks() {
    let (e, time) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    e.store
        .put(0, "menu:7", &json!({"user":7,"applied":true,"version":3}))
        .unwrap();
    admin::handle(&e, 0, &admin_message(7, "/stats"))
        .await
        .unwrap();
    let commands = e.services.calls("setMyCommands");
    assert_eq!(commands.len(), 1);
    assert!(
        commands[0]["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["command"] == "stats")
    );
    assert_eq!(commands[0]["scope"]["chat_id"], 7);
    let thresholds = commands[0]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["command"] == "thresholds")
        .unwrap();
    assert_eq!(
        thresholds["description"],
        "阈值： /thresholds CHAT_ID 60 80"
    );
    menus::set_private(&e, 7, true).await.unwrap();
    assert_eq!(e.services.calls("setMyCommands").len(), 1);
    assert!(
        e.services.calls("sendMessage")[0]["reply_markup"]
            .to_string()
            .contains(&format!("stats|{CHAT}|"))
    );
    time.fetch_add(86400, Ordering::SeqCst);
    admin::handle(
        &e,
        CHAT,
        &admin_callback(7, CHAT, "stats", &format!("d:{}", day(NOW))),
    )
    .await
    .unwrap();
    let replies = e.services.replies();
    for reply in &replies {
        assert!(formatting::rendered(reply).encode_utf16().count() <= 4096);
        for row in reply["reply_markup"]["inline_keyboard"].as_array().unwrap() {
            for b in row.as_array().unwrap() {
                assert!(b["callback_data"].as_str().unwrap().len() <= 64);
            }
        }
    }
    assert!(
        replies.last().unwrap()["reply_markup"]
            .to_string()
            .contains("下一天")
    );
    assert_chinese_admin_replies(&e);
}

#[test]
fn dates_timezone_zero_usage_and_historical_coverage_are_explicit() {
    let (e, _) = setup(None);
    let midnight = (day(NOW) + 1) * 86400 - 28800;
    assert_eq!(day(midnight), day(midnight - 1) + 1);
    assert!(e.store.statistics_date("2024-02-29").unwrap().is_some());
    assert_eq!(e.store.statistics_date("2026-02-29").unwrap(), None);
    assert_eq!(e.store.statistics_date("2026-02-30").unwrap(), None);
    let old = report(&e, NOW - 86400);
    assert!(old.partial);
    assert_eq!(old.attempts, 0);
    let id = e.store.start_usage(CHAT, NOW).unwrap();
    e.store
        .finish_usage(
            CHAT,
            &id,
            &Usage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                cost_usd: Some(0.0),
            },
        )
        .unwrap();
    let current = report(&e, NOW);
    assert_eq!(
        (
            current.input_known,
            current.output_known,
            current.cost_known
        ),
        (1, 1, 1)
    );
}
