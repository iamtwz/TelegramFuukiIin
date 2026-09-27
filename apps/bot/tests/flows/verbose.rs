use super::*;
use fuuki_iin_bot::statistics::{Report, day};

const TARGET: i64 = 123456;

fn verbose_setup(p: Option<f64>) -> (Engine<Mock>, Arc<AtomicI64>) {
    let (mut e, time) = setup(p);
    Arc::make_mut(&mut e.config).verbose_chats = vec![CHAT];
    (e, time)
}

fn command(id: i64, text: &str) -> Value {
    let mut m = message(id);
    m["from"]["is_bot"] = json!(false);
    m["text"] = json!(text);
    m
}

fn ingest(e: &Engine<Mock>, id: i64, m: &Value) {
    e.ingest(&json!({"update_id":id,"message":m})).unwrap();
}

fn assessment_replies(e: &Engine<Mock>) -> Vec<Value> {
    e.services
        .calls("sendMessage")
        .into_iter()
        .filter(|reply| {
            let text = reply["text"].as_str().unwrap_or("");
            reply["chat_id"] == CHAT && text.contains("Jev 判断") && text.contains("Jev assessment")
        })
        .collect()
}

fn assert_assessment(reply: &Value, result: &str) {
    assert_eq!(reply["chat_id"], CHAT);
    assert_eq!(reply["parse_mode"], "MarkdownV2");
    let text = reply["text"].as_str().unwrap();
    assert!(text.contains("Jev 判断 / Jev assessment"));
    assert!(text.contains("Spam 概率 / Spam probability"));
    assert!(text.contains("test-model") || text.contains("test\\-model"));
    assert!(text.contains(result));
    assert!(text.encode_utf16().count() <= 4096);
}

fn report(e: &Engine<Mock>) -> Report {
    e.store
        .statistics_report(CHAT, day(NOW), day(NOW), e.now())
        .unwrap()
}

fn current_member(e: &Engine<Mock>, id: i64) {
    e.services.member(
        id,
        json!({"status":"member","user":{"id":id,"is_bot":false,"first_name":"测试用户"}}),
    );
}

fn profile(e: &Engine<Mock>, id: i64, bio: &str) {
    e.services.chats.lock().unwrap().insert(
        id,
        json!({"id":id,"type":"private","first_name":"目标用户","username":"synthetic_target","bio":bio}),
    );
}

fn no_punishment(e: &Engine<Mock>) {
    for method in ["banChatMember", "deleteMessage", "approveChatJoinRequest"] {
        assert!(e.services.calls(method).is_empty(), "unexpected {method}");
    }
}

#[tokio::test]
async fn automatic_success_notifies_verbose_group_once_for_every_risk_level() {
    for (probability, result) in [(0.1, "低风险"), (0.7, "人工复核"), (0.95, "高风险")] {
        let (e, _) = verbose_setup(Some(probability));
        e.joined(CHAT, USER, NOW, true).unwrap();
        e.message(CHAT, &message(700)).unwrap();
        e.message(CHAT, &message(700)).unwrap();
        drain(&e).await;
        let replies = assessment_replies(&e);
        assert_eq!(replies.len(), 1);
        assert_assessment(&replies[0], result);
        assert!(replies[0]["text"].as_str().unwrap().contains('%'));
        assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
        let id = cases(&e)[0].id.clone();
        e.queue(
            CHAT,
            "classify",
            "duplicate-verbose-result",
            json!({"id":id}),
        )
        .unwrap();
        drain(&e).await;
        assert_eq!(assessment_replies(&e).len(), 1);
        assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn ordinary_groups_and_protected_admin_cases_send_no_verbose_assessment() {
    for protected in [false, true] {
        let (mut e, _) = verbose_setup(Some(0.1));
        if protected {
            e.services.member(USER, json!({"status":"creator"}));
        } else {
            Arc::make_mut(&mut e.config).verbose_chats.clear();
        }
        e.joined(CHAT, USER, NOW, true).unwrap();
        e.message(CHAT, &message(701)).unwrap();
        drain(&e).await;
        assert!(assessment_replies(&e).is_empty());
        assert_eq!(
            e.services.evidence.lock().unwrap().len(),
            usize::from(!protected)
        );
    }
}

#[tokio::test]
async fn join_profile_and_guest_classifications_also_notify_the_verbose_group() {
    for join_profile in [true, false] {
        let (e, _) = verbose_setup(Some(0.1));
        if join_profile {
            let mut settings = e.settings(CHAT).unwrap();
            settings.join_profile_review = true;
            e.store.put(CHAT, "settings", &settings).unwrap();
            profile(&e, USER, "合成申请资料 @synthetic_contact");
            e.join_request(CHAT, &join(NOW), 702).unwrap();
        } else {
            let guest = json!({"message_id":702,"date":NOW,"chat":{"id":CHAT,"type":"supergroup"},"from":{"id":88,"is_bot":true,"username":"guest_bot"},"guest_bot_caller_user":{"id":USER,"is_bot":false},"text":"合成 Guest 内容","reply_to_message":{"message_id":701,"chat":{"id":CHAT},"from":{"id":USER,"is_bot":false},"text":"@guest_bot"}});
            e.message(CHAT, &guest).unwrap();
        }
        drain(&e).await;
        assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
        let replies = assessment_replies(&e);
        assert_eq!(replies.len(), 1);
        assert_assessment(&replies[0], "低风险");
        assert_eq!(cases(&e).len(), 1);
    }
}

#[tokio::test]
async fn automatic_notice_retry_does_not_repeat_the_model_call() {
    let (e, time) = verbose_setup(Some(0.1));
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("sendMessage".into(), 1);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(703)).unwrap();
    drain(&e).await;
    assert_eq!(assessment_replies(&e).len(), 1);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    *e.services.probability.lock().unwrap() = Some(0.95);
    time.fetch_add(60, Ordering::SeqCst);
    drain(&e).await;
    let replies = assessment_replies(&e);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0], replies[1]);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(report(&e).attempts, 1);
}

#[tokio::test]
async fn manual_text_preserves_raw_content_and_never_consumes_or_punishes_first_message() {
    let (e, _) = verbose_setup(Some(0.95));
    current_member(&e, USER);
    e.joined(CHAT, USER, NOW, true).unwrap();
    *e.services.usage.lock().unwrap() = Usage {
        input_tokens: Some(20),
        output_tokens: Some(2),
        cost_usd: Some(0.000001),
    };
    let raw = "  第一行 *合成内容*\n  第二行  保留空格\n";
    let m = command(800, &format!("/spamcheck text {raw}"));
    ingest(&e, 800, &m);
    ingest(&e, 800, &m);
    drain(&e).await;
    let evidence = e.services.evidence.lock().unwrap().clone();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["message"]["text"], raw);
    assert_eq!(evidence[0]["context"], "group_message");
    assert!(evidence[0]["user"].is_null());
    assert!(cases(&e).is_empty());
    assert_eq!(
        e.store
            .get::<Member>(CHAT, &format!("member:{USER}"))
            .unwrap()
            .unwrap()
            .first_message,
        None
    );
    let r = report(&e);
    assert_eq!(
        (r.messages, r.profiles, r.spam, r.classified, r.attempts),
        (0, 0, 0, 0, 1)
    );
    assert_eq!((r.input, r.output), (20, 2));
    no_punishment(&e);
    let replies = assessment_replies(&e);
    assert_eq!(replies.len(), 1);
    assert_assessment(&replies[0], "高风险");
    assert_eq!(replies[0]["reply_parameters"]["message_id"], 800);
    e.message(CHAT, &message(801)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e).len(), 1);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn manual_id_uses_confirmed_profile_and_addressed_group_command() {
    let (e, _) = verbose_setup(Some(0.7));
    current_member(&e, USER);
    current_member(&e, TARGET);
    profile(
        &e,
        TARGET,
        "目标 BIO @synthetic_contact t.me/synthetic_target",
    );
    let mut m = command(802, "/spamcheck@FuukiIinTestBot id 123456");
    m["chat"]["type"] = json!("group");
    m["message_thread_id"] = json!(19);
    ingest(&e, 802, &m);
    drain(&e).await;
    let evidence = e.services.evidence.lock().unwrap().clone();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["context"], "join_profile");
    assert_eq!(evidence[0]["user"]["id"], TARGET);
    assert_eq!(
        evidence[0]["profiles"]["user"]["identity"]["username"],
        "synthetic_target"
    );
    assert_eq!(
        evidence[0]["profiles"]["user"]["bio"]["mentions"],
        json!(["@synthetic_contact"])
    );
    let replies = assessment_replies(&e);
    assert_eq!(replies.len(), 1);
    assert_assessment(&replies[0], "人工复核");
    assert_eq!(replies[0]["message_thread_id"], 19);
    assert!(cases(&e).is_empty());
    no_punishment(&e);
    assert_eq!(
        (
            report(&e).messages,
            report(&e).profiles,
            report(&e).attempts
        ),
        (0, 0, 1)
    );
}

#[tokio::test]
async fn manual_command_rejects_untrusted_senders_and_wrong_contexts() {
    for mode in [
        "other_bot",
        "sender_bot",
        "sender_chat",
        "private",
        "edited",
        "ordinary",
        "unmanaged",
        "channel",
        "left",
        "restricted_absent",
    ] {
        let (mut e, _) = verbose_setup(Some(0.95));
        current_member(&e, USER);
        let mut m = command(804, "/spamcheck text 合成内容");
        match mode {
            "other_bot" => m["text"] = json!("/spamcheck@OtherTestBot text 合成内容"),
            "sender_bot" => m["from"]["is_bot"] = json!(true),
            "sender_chat" => m["sender_chat"] = json!({"id":CHAT,"type":"supergroup"}),
            "private" => m["chat"] = json!({"id":USER,"type":"private"}),
            "ordinary" => Arc::make_mut(&mut e.config).verbose_chats.clear(),
            "unmanaged" => Arc::make_mut(&mut e.config).chats.clear(),
            "channel" => m["chat"]["type"] = json!("channel"),
            "left" => e.services.member(USER, json!({"status":"left"})),
            "restricted_absent" => e
                .services
                .member(USER, json!({"status":"restricted","is_member":false})),
            "edited" => (),
            _ => unreachable!(),
        }
        if mode == "edited" {
            e.ingest(&json!({"update_id":804,"edited_message":m}))
                .unwrap();
        } else {
            ingest(&e, 804, &m);
        }
        drain(&e).await;
        assert!(
            e.services.evidence.lock().unwrap().is_empty(),
            "unexpected model call for {mode}"
        );
        assert!(cases(&e).is_empty(), "unexpected case for {mode}");
        assert!(
            assessment_replies(&e).is_empty(),
            "unexpected assessment for {mode}"
        );
        no_punishment(&e);
    }
}

#[tokio::test]
async fn unknown_target_identity_replies_without_calling_the_model() {
    let (e, _) = verbose_setup(Some(0.95));
    current_member(&e, USER);
    e.services.member(TARGET, json!({"status":"left"}));
    ingest(&e, 805, &command(805, "/spamcheck id 123456"));
    drain(&e).await;
    assert!(e.services.evidence.lock().unwrap().is_empty());
    assert!(cases(&e).is_empty());
    assert_eq!(report(&e).attempts, 0);
    let replies = e.services.calls("sendMessage");
    assert!(!replies.is_empty());
    assert!(replies.iter().all(|reply| reply["chat_id"] == CHAT));
    no_punishment(&e);
}

#[tokio::test]
async fn current_member_identity_is_a_valid_fallback_when_get_chat_is_unavailable() {
    let (e, _) = verbose_setup(Some(0.1));
    current_member(&e, USER);
    current_member(&e, TARGET);
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChat".into(), 1);
    ingest(&e, 806, &command(806, "/spamcheck id 123456"));
    drain(&e).await;
    let evidence = e.services.evidence.lock().unwrap().clone();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["user"]["id"], TARGET);
    assert!(evidence[0]["profiles"]["user"]["bio"].is_null());
    assert_eq!(assessment_replies(&e).len(), 1);
    no_punishment(&e);
}

#[tokio::test]
async fn manual_retries_preserve_evidence_across_restart_and_stop_after_three_calls() {
    let (mut e, time) = verbose_setup(None);
    let dir = std::env::temp_dir().join(format!("fuuki-iin-manual-jev-{}", uuid::Uuid::new_v4()));
    let db = dir.join("bot.sqlite");
    e.store = Arc::new(Store::open(&db).unwrap());
    current_member(&e, USER);
    current_member(&e, TARGET);
    profile(&e, TARGET, "原始 BIO @original_contact");
    *e.services.classify_transport_failures.lock().unwrap() = 1;
    *e.services.usage.lock().unwrap() = Usage {
        input_tokens: Some(20),
        output_tokens: Some(2),
        cost_usd: Some(0.000001),
    };
    let m = command(807, "/spamcheck id 123456");
    ingest(&e, 807, &m);
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    let records = e
        .store
        .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["state"], "pending");
    assert_eq!(
        records[0]["evidence"],
        e.services.evidence.lock().unwrap()[0]
    );
    profile(&e, TARGET, "重试期间变化的 BIO @changed_contact");
    e.store = Arc::new(Store::memory().unwrap());
    e.store = Arc::new(Store::open(&db).unwrap());
    e.store.recover(e.now()).unwrap();
    ingest(&e, 807, &m);
    for _ in 0..3 {
        time.fetch_add(60, Ordering::SeqCst);
        drain(&e).await;
    }
    let evidence = e.services.evidence.lock().unwrap().clone();
    assert_eq!(evidence.len(), 3);
    assert!(evidence.iter().all(|data| data == &evidence[0]));
    let replies = assessment_replies(&e);
    assert_eq!(replies.len(), 1);
    let text = replies[0]["text"].as_str().unwrap();
    assert!(text.contains("未知"));
    assert!(text.contains("Unknown") || text.contains("unknown"));
    assert!(!text.contains('%'));
    assert!(cases(&e).is_empty());
    no_punishment(&e);
    let r = report(&e);
    assert_eq!(
        (r.messages, r.profiles, r.failed, r.attempts, r.input_known),
        (0, 0, 0, 3, 2)
    );
    assert_eq!((r.input, r.output), (40, 4));
    let done = e
        .store
        .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
        .unwrap();
    assert_eq!(done[0]["state"], "done");
    assert!(done[0]["evidence"].is_null());
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn manual_notice_retry_reuses_result_and_deduplicates_the_original_update() {
    let (e, time) = verbose_setup(Some(0.1));
    current_member(&e, USER);
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("sendMessage".into(), 1);
    let m = command(808, "/spamcheck text 合成内容");
    ingest(&e, 808, &m);
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(assessment_replies(&e).len(), 1);
    *e.services.probability.lock().unwrap() = Some(0.95);
    ingest(&e, 808, &m);
    time.fetch_add(60, Ordering::SeqCst);
    drain(&e).await;
    let replies = assessment_replies(&e);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0], replies[1]);
    assert_assessment(&replies[1], "低风险");
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(report(&e).attempts, 1);
    assert!(cases(&e).is_empty());
    no_punishment(&e);
}

#[tokio::test]
async fn manual_per_user_cooldown_allows_the_ten_second_boundary() {
    let (e, time) = verbose_setup(Some(0.1));
    current_member(&e, USER);
    for (id, offset, expected) in [(810, 0, 1), (811, 9, 1), (812, 10, 2)] {
        time.store(NOW + offset, Ordering::SeqCst);
        let mut m = command(id, "/spamcheck text 合成内容");
        m["date"] = json!(e.now());
        ingest(&e, id, &m);
        drain(&e).await;
        assert_eq!(e.services.evidence.lock().unwrap().len(), expected);
    }
    assert_eq!(report(&e).attempts, 2);
    assert!(cases(&e).is_empty());
    no_punishment(&e);
}

#[tokio::test]
async fn manual_group_limit_is_shared_by_users_and_resets_after_a_minute() {
    let (e, time) = verbose_setup(Some(0.1));
    for i in 0..11 {
        let user = USER + i;
        current_member(&e, user);
        let mut m = command(820 + i, "/spamcheck text 合成内容");
        m["from"]["id"] = json!(user);
        ingest(&e, 820 + i, &m);
        drain(&e).await;
        assert_eq!(
            e.services.evidence.lock().unwrap().len(),
            (i + 1).min(10) as usize
        );
    }
    assert_eq!(report(&e).attempts, 10);
    time.store(NOW + 60, Ordering::SeqCst);
    let mut m = command(840, "/spamcheck text 新一分钟的合成内容");
    m["from"]["id"] = json!(USER + 10);
    m["date"] = json!(e.now());
    ingest(&e, 840, &m);
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 11);
    assert_eq!(report(&e).attempts, 11);
    assert!(cases(&e).is_empty());
    no_punishment(&e);
}

#[tokio::test]
async fn malformed_manual_inputs_never_call_the_model_or_consume_first_message() {
    for text in [
        "/spamcheck",
        "/spamcheck id",
        "/spamcheck id 0",
        "/spamcheck id -1",
        "/spamcheck id 9007199254740992",
        "/spamcheck id 18446744073709551616",
        "/spamcheck text",
        "/spamcheck text \n\t  ",
    ] {
        let (e, _) = verbose_setup(Some(0.95));
        current_member(&e, USER);
        e.joined(CHAT, USER, NOW, true).unwrap();
        ingest(&e, 850, &command(850, text));
        drain(&e).await;
        assert!(
            e.services.evidence.lock().unwrap().is_empty(),
            "unexpected model call for {text:?}"
        );
        assert!(cases(&e).is_empty());
        assert_eq!(report(&e).attempts, 0);
        assert_eq!(
            e.store
                .get::<Member>(CHAT, &format!("member:{USER}"))
                .unwrap()
                .unwrap()
                .first_message,
            None
        );
        no_punishment(&e);
    }
}

#[tokio::test]
async fn removing_managed_group_stops_queued_models_and_notices() {
    for phase in [
        "automatic_model",
        "automatic_notice",
        "manual_model",
        "manual_notice",
    ] {
        let (mut e, _) = verbose_setup(Some(0.1));
        current_member(&e, USER);
        if phase.starts_with("automatic") {
            e.joined(CHAT, USER, NOW, true).unwrap();
            e.message(CHAT, &message(851)).unwrap();
        } else {
            ingest(&e, 851, &command(851, "/spamcheck text 合成内容"));
        }
        if phase.ends_with("notice") {
            let job = e.store.claim(e.now()).unwrap().unwrap();
            assert_eq!(
                job.kind,
                if phase.starts_with("automatic") {
                    "classify"
                } else {
                    "manual_jev"
                }
            );
            e.run(&job).await.unwrap();
            assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
        }
        e.services.calls.lock().unwrap().clear();
        e.services.evidence.lock().unwrap().clear();
        Arc::make_mut(&mut e.config).chats.clear();
        drain(&e).await;
        assert!(
            e.services.calls.lock().unwrap().is_empty(),
            "unexpected API call for {phase}"
        );
        assert!(
            e.services.evidence.lock().unwrap().is_empty(),
            "unexpected model call for {phase}"
        );
    }
}

#[tokio::test]
async fn disabling_verbose_stops_queued_manual_models_and_all_verbose_notices() {
    for phase in ["automatic_notice", "manual_model", "manual_notice"] {
        let (mut e, _) = verbose_setup(Some(0.1));
        current_member(&e, USER);
        if phase.starts_with("automatic") {
            e.joined(CHAT, USER, NOW, true).unwrap();
            e.message(CHAT, &message(852)).unwrap();
        } else {
            ingest(&e, 852, &command(852, "/spamcheck text 合成内容"));
        }
        if phase.ends_with("notice") {
            let job = e.store.claim(e.now()).unwrap().unwrap();
            e.run(&job).await.unwrap();
            assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
        }
        e.services.calls.lock().unwrap().clear();
        e.services.evidence.lock().unwrap().clear();
        Arc::make_mut(&mut e.config).verbose_chats.clear();
        drain(&e).await;
        assert!(
            e.services.calls.lock().unwrap().is_empty(),
            "unexpected API call for {phase}"
        );
        assert!(
            e.services.evidence.lock().unwrap().is_empty(),
            "unexpected model call for {phase}"
        );
    }
}

#[tokio::test]
async fn disabling_verbose_keeps_normal_automatic_screening_enabled() {
    let (mut e, _) = verbose_setup(Some(0.1));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(853)).unwrap();
    Arc::make_mut(&mut e.config).verbose_chats.clear();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(cases(&e)[0].state, "allowed");
    assert!(assessment_replies(&e).is_empty());
}

#[tokio::test]
async fn exhausted_automatic_model_calls_send_one_unknown_assessment() {
    let (e, time) = verbose_setup(None);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(854)).unwrap();
    for _ in 0..3 {
        drain(&e).await;
        time.fetch_add(60, Ordering::SeqCst);
    }
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 3);
    let replies = assessment_replies(&e);
    assert_eq!(replies.len(), 1);
    assert_assessment(&replies[0], "未知");
    let text = replies[0]["text"].as_str().unwrap();
    assert!(text.contains("Unknown"));
    assert!(!text.contains('%'));
    assert_eq!(cases(&e)[0].state, "review");
    assert!(cases(&e)[0].probability.is_none());
    assert_eq!(report(&e).attempts, 3);
    no_punishment(&e);
}

#[tokio::test]
async fn exhausted_permission_queries_without_model_calls_send_no_assessment() {
    let (e, time) = verbose_setup(Some(0.95));
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChatMember".into(), 3);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(855)).unwrap();
    for _ in 0..3 {
        drain(&e).await;
        time.fetch_add(60, Ordering::SeqCst);
    }
    drain(&e).await;
    assert_eq!(e.services.calls("getChatMember").len(), 3);
    assert!(e.services.evidence.lock().unwrap().is_empty());
    assert!(assessment_replies(&e).is_empty());
    assert_eq!(report(&e).attempts, 0);
    assert_eq!(cases(&e)[0].state, "review");
    no_punishment(&e);
}

#[tokio::test]
async fn manual_bot_id_preserves_confirmed_bot_identity() {
    let (e, _) = verbose_setup(Some(0.1));
    current_member(&e, USER);
    e.services.member(TARGET, json!({"status":"member","user":{"id":TARGET,"is_bot":true,"username":"SyntheticTargetBot","first_name":"测试 Bot"}}));
    profile(&e, TARGET, "合成 Bot BIO @synthetic_contact");
    ingest(&e, 856, &command(856, "/spamcheck id 123456"));
    drain(&e).await;
    let evidence = e.services.evidence.lock().unwrap().clone();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["user"]["id"], TARGET);
    assert_eq!(evidence[0]["user"]["is_bot"], true);
    assert_eq!(evidence[0]["profiles"]["user"]["identity"]["is_bot"], true);
    assert_eq!(assessment_replies(&e).len(), 1);
    assert!(cases(&e).is_empty());
    no_punishment(&e);
}

#[tokio::test]
async fn spamcheck_menu_is_only_advertised_in_configured_verbose_group_scope() {
    let (mut e, _) = verbose_setup(None);
    let ordinary = CHAT - 1;
    Arc::make_mut(&mut e.config).chats.push(ordinary);
    menus::install_public(e.services.as_ref(), &e.config)
        .await
        .unwrap();
    menus::set_private(&e, USER, true).await.unwrap();
    menus::set_private(&e, USER, false).await.unwrap();
    let calls = e.services.calls("setMyCommands");
    let mut advertised = 0;
    for call in &calls {
        let has_spamcheck = call["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["command"] == "spamcheck");
        assert_eq!(
            has_spamcheck,
            call["scope"] == json!({"type":"chat","chat_id":CHAT})
        );
        if has_spamcheck {
            advertised += 1;
        }
    }
    assert_eq!(advertised, 1);
    assert!(
        calls
            .iter()
            .any(|call| call["scope"] == json!({"type":"chat","chat_id":ordinary}))
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["scope"]["chat_id"] == USER)
            .count(),
        2
    );
    e.services.calls.lock().unwrap().clear();
    Arc::make_mut(&mut e.config).verbose_chats.clear();
    menus::install_public(e.services.as_ref(), &e.config)
        .await
        .unwrap();
    assert!(e.services.calls("setMyCommands").iter().all(|call| {
        call["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["command"] != "spamcheck")
    }));
}

#[tokio::test]
async fn expired_manual_retry_completes_without_another_call_and_clears_raw_evidence() {
    let (e, time) = verbose_setup(Some(0.1));
    current_member(&e, USER);
    *e.services.classify_transport_failures.lock().unwrap() = 1;
    let raw = "合成请求原文 @expired_snapshot";
    ingest(&e, 860, &command(860, &format!("/spamcheck text {raw}")));
    drain(&e).await;
    let pending = e
        .store
        .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["state"], "pending");
    assert_eq!(pending[0]["evidence"]["message"]["text"], raw);
    assert_eq!(pending[0]["created_at"], NOW);
    time.store(NOW + 301, Ordering::SeqCst);
    drain(&e).await;
    let done = e
        .store
        .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
        .unwrap();
    assert_eq!(done[0]["state"], "done");
    assert!(done[0]["evidence"].is_null());
    assert!(!done[0].to_string().contains(raw));
    assert_eq!(done[0]["created_at"], NOW);
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(report(&e).attempts, 1);
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 1);
    assert!(replies[0]["text"].as_str().unwrap().contains("请求已过期"));
    assert!(
        replies[0]["text"]
            .as_str()
            .unwrap()
            .contains("Request expired")
    );
    no_punishment(&e);
}

#[tokio::test]
async fn cancelled_manual_retry_marks_done_and_clears_persisted_evidence() {
    for mode in ["verbose_disabled", "managed_removed", "sender_left"] {
        let (mut e, time) = verbose_setup(Some(0.1));
        current_member(&e, USER);
        *e.services.classify_transport_failures.lock().unwrap() = 1;
        let raw = "合成请求原文 @cancelled_snapshot";
        ingest(&e, 861, &command(861, &format!("/spamcheck text {raw}")));
        drain(&e).await;
        let pending = e
            .store
            .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["state"], "pending");
        assert_eq!(pending[0]["evidence"]["message"]["text"], raw);
        match mode {
            "verbose_disabled" => Arc::make_mut(&mut e.config).verbose_chats.clear(),
            "managed_removed" => Arc::make_mut(&mut e.config).chats.clear(),
            "sender_left" => e.services.member(USER, json!({"status":"left"})),
            _ => unreachable!(),
        }
        time.store(NOW + 60, Ordering::SeqCst);
        drain(&e).await;
        let done = e
            .store
            .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
            .unwrap();
        assert_eq!(done[0]["state"], "done", "not completed for {mode}");
        assert!(
            done[0]["evidence"].is_null(),
            "evidence retained for {mode}"
        );
        assert!(!done[0].to_string().contains(raw));
        assert_eq!(done[0]["created_at"], NOW);
        assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
        assert_eq!(report(&e).attempts, 1);
        assert!(e.services.calls("sendMessage").is_empty());
        no_punishment(&e);
    }
}

#[tokio::test]
async fn prune_expires_manual_records_after_seven_days_and_limits_after_one_day() {
    let (e, time) = verbose_setup(Some(0.1));
    current_member(&e, USER);
    current_member(&e, USER + 1);
    *e.services.classify_transport_failures.lock().unwrap() = 1;
    ingest(&e, 862, &command(862, "/spamcheck text 合成待重试原文"));
    drain(&e).await;
    let mut completed = command(863, "/spamcheck text 合成成功原文");
    completed["from"]["id"] = json!(USER + 1);
    ingest(&e, 863, &completed);
    drain(&e).await;
    let records = e
        .store
        .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record["state"] == "pending" && record["evidence"].is_object())
    );
    assert!(
        records
            .iter()
            .any(|record| record["state"] == "done" && record["evidence"].is_null())
    );
    assert_eq!(
        e.store
            .list::<Value>(CHAT, "jev_limit:", None, 0, 100)
            .unwrap()
            .len(),
        3
    );
    time.store(NOW + 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert_eq!(
        e.store
            .list::<Value>(CHAT, "jev_limit:", None, 0, 100)
            .unwrap()
            .len(),
        3
    );
    time.store(NOW + 86401, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert!(
        e.store
            .list::<Value>(CHAT, "jev_limit:", None, 0, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        e.store
            .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
            .unwrap()
            .len(),
        2
    );
    time.store(NOW + 7 * 86400, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert_eq!(
        e.store
            .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
            .unwrap()
            .len(),
        2
    );
    time.fetch_add(1, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert!(
        e.store
            .list::<Value>(CHAT, "manual_jev:", None, 0, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(e.services.evidence.lock().unwrap().len(), 2);
    no_punishment(&e);
}
