use super::*;
use fuuki_iin_bot::{
    model::{CaseKind, ScreeningMode, Settings},
    profiles,
    statistics::day,
};

const GUEST: i64 = 88;
fn profile(e: &Engine<Mock>, id: i64, text: &str) {
    e.services.chats.lock().unwrap().insert(id,json!({"id":id,"type":"private","username":"current_name","first_name":"当前昵称","bio":text,"unneeded_private_data":"must-not-leak"}));
}
fn enable_join_profiles(e: &Engine<Mock>) {
    let mut s = e.settings(CHAT).unwrap();
    s.join_profile_review = true;
    e.store.put(CHAT, "settings", &s).unwrap();
}
fn guest(id: i64, caller: i64) -> Value {
    json!({"message_id":id,"date":NOW,"chat":{"id":CHAT,"type":"supergroup"},"from":{"id":GUEST,"is_bot":true,"username":"guest_bot"},"guest_bot_caller_user":{"id":caller,"is_bot":false,"username":"caller"},"text":"💰","reply_markup":{"inline_keyboard":[[{"text":"一天八万","url":"https://t.me/spam_channel"}]]},"reply_to_message":{"message_id":id-1,"chat":{"id":CHAT},"from":{"id":caller,"is_bot":false},"text":"@guest_bot"}})
}
fn bans(e: &Engine<Mock>) -> Vec<i64> {
    e.services
        .calls("banChatMember")
        .iter()
        .map(|m| {
            assert_eq!(m["revoke_messages"], true);
            m["user_id"].as_i64().unwrap()
        })
        .collect()
}
fn text(e: &Engine<Mock>) -> String {
    e.services
        .replies()
        .iter()
        .map(super::formatting::rendered)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn guest_threshold_boundaries_apply_to_bot_and_confirmed_caller() {
    for (probability, expected) in [
        (0.599999, "allowed"),
        (0.6, "review"),
        (0.799999, "review"),
        (0.8, "banned"),
    ] {
        let (e, _) = setup(Some(probability));
        e.message(CHAT, &guest(2, USER)).unwrap();
        drain(&e).await;
        assert_eq!(cases(&e)[0].state, expected);
        assert_eq!(cases(&e)[0].ban_threshold, 0.8);
        if expected == "banned" {
            assert_eq!(bans(&e), vec![GUEST, USER]);
            let deleted: Vec<_> = e
                .services
                .calls("deleteMessage")
                .iter()
                .map(|m| m["message_id"].as_i64().unwrap())
                .collect();
            assert_eq!(deleted, vec![2, 1]);
        } else {
            assert!(bans(&e).is_empty());
            assert!(e.services.calls("deleteMessage").is_empty());
        }
    }
}

#[tokio::test]
async fn default_join_and_old_settings_cost_no_jev_calls() {
    let (e, _) = setup(Some(0.99));
    let old: Settings =
        serde_json::from_value(json!({"spam":true,"captcha":true,"review":0.6,"ban":0.9})).unwrap();
    assert!(!old.join_profile_review);
    assert!(!e.settings(CHAT).unwrap().join_profile_review);
    let mut r = join(NOW);
    r["bio"] = json!("合成 BIO @test_user");
    e.join_request(CHAT, &r, 1).unwrap();
    drain(&e).await;
    assert!(cases(&e).is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
    assert!(e.services.calls("getChat").is_empty());
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    drain(&e).await;
    assert_eq!(session(&e).state, "approved");
    assert!(e.services.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn message_sends_current_bio_links_mentions_and_inline_bot_profile_once_per_case() {
    let (e, time) = setup(None);
    profile(
        &e,
        USER,
        "联系 @scam_user，保本高收益 https://t.me/scam_channel",
    );
    profile(&e, GUEST, "查看 t.me/bot_home @bot_contact");
    e.joined(CHAT, USER, NOW, true).unwrap();
    let mut m = message(1);
    m["via_bot"] = json!({"id":GUEST,"is_bot":true});
    e.message(CHAT, &m).unwrap();
    drain(&e).await;
    profile(&e, USER, "changed during retry");
    *e.services.probability.lock().unwrap() = Some(0.99);
    time.fetch_add(10, Ordering::SeqCst);
    drain(&e).await;
    let sent = e.services.evidence.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    assert_eq!(
        sent[0]["profiles"]["user"]["identity"]["username"],
        "current_name"
    );
    assert_eq!(
        sent[0]["profiles"]["user"]["bio"]["mentions"],
        json!(["@scam_user"])
    );
    assert_eq!(
        sent[0]["profiles"]["user"]["bio"]["telegram_links"],
        json!(["https://t.me/scam_channel"])
    );
    assert_eq!(
        sent[0]["profiles"]["via_bot"]["bio"]["mentions"],
        json!(["@bot_contact"])
    );
    assert!(!sent[0].to_string().contains("must-not-leak"));
    assert_eq!(cases(&e)[0].evidence.as_ref(), Some(&sent[0]));
    assert_eq!(e.services.calls("getChat").len(), 2);
    assert_eq!(bans(&e), vec![USER]);
    let request = evidence::classify_request(&sent[0], "test-model").unwrap();
    assert!(
        request["questions"]["spam"]["instructions"]
            .as_str()
            .unwrap()
            .contains("BIO")
    );
}

#[tokio::test]
async fn unavailable_bio_uses_labeled_join_snapshot_without_inventing_current_bio() {
    let (e, _) = setup(Some(0.1));
    let mut r = join(NOW);
    r["bio"] = json!("申请时 @application_name t.me/application");
    e.join_request(CHAT, &r, 1).unwrap();
    drain(&e).await;
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    drain(&e).await;
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChat".into(), 1);
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let data = e.services.evidence.lock().unwrap()[0].clone();
    let p = &data["profiles"]["user"];
    assert_eq!(p["lookup"], "unavailable");
    assert!(p["bio"].is_null());
    assert_eq!(
        p["join_request"]["bio"]["mentions"],
        json!(["@application_name"])
    );
    assert_eq!(p["join_request"]["source"], "ChatJoinRequest");
    assert_eq!(cases(&e)[0].state, "allowed");
}

#[test]
fn bio_extraction_is_bounded_unicode_safe_and_does_not_treat_email_as_a_mention() {
    let p = profiles::bio(
        "找@abc_user。email a@example.com；https://t.me/abc?start=x。t.me/+invite https://t.me.evil/bad nott.me/bad @abc_user TG://resolve?domain=test",
    );
    assert_eq!(p["mentions"], json!(["@abc_user"]));
    assert_eq!(
        p["telegram_links"],
        json!([
            "https://t.me/abc?start=x",
            "t.me/+invite",
            "TG://resolve?domain=test"
        ])
    );
    let long = profiles::bio(&"@abc ".repeat(2000));
    assert_eq!(long["text"].as_str().unwrap().chars().count(), 4096);
    assert_eq!(long["truncated"], true);
    assert_eq!(long["mentions"], json!(["@abc"]));
}

#[tokio::test]
async fn mismatched_profile_response_never_replaces_the_sender_or_exposes_unrelated_bio() {
    let (e, _) = setup(Some(0.95));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.services.chats.lock().unwrap().insert(
        USER,
        json!({"id":999,"type":"private","bio":"unrelated secret"}),
    );
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    assert!(
        !e.services.evidence.lock().unwrap()[0]
            .to_string()
            .contains("unrelated secret")
    );
    assert_eq!(bans(&e), vec![USER]);
}

#[tokio::test]
async fn guest_replies_screen_each_message_even_after_callers_first_message_and_deduplicate_updates()
 {
    let (e, _) = setup(Some(0.01));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    *e.services.probability.lock().unwrap() = Some(0.99);
    profile(&e, USER, "@caller_contact t.me/caller");
    profile(&e, GUEST, "@guest_contact");
    for (id, caller) in [(2, USER), (4, 43)] {
        let m = guest(id, caller);
        e.message(CHAT, &m).unwrap();
        e.message(CHAT, &m).unwrap();
        drain(&e).await;
    }
    assert_eq!(e.services.evidence.lock().unwrap().len(), 3);
    assert_eq!(bans(&e), vec![GUEST, USER, GUEST, 43]);
    let sent = e.services.evidence.lock().unwrap()[1].clone();
    assert_eq!(sent["guest_bot_caller_user"]["id"], USER);
    assert_eq!(
        sent["profiles"]["guest_bot_caller_user"]["bio"]["mentions"],
        json!(["@caller_contact"])
    );
    assert_eq!(
        sent["message"]["reply_markup"],
        guest(2, USER)["reply_markup"]
    );
    assert_eq!(
        e.services
            .calls("deleteMessage")
            .iter()
            .map(|m| m["message_id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 1, 4, 3]
    );
    let stats = e
        .store
        .statistics_report(CHAT, day(NOW), day(NOW), NOW)
        .unwrap();
    assert_eq!((stats.messages, stats.spam, stats.profiles), (3, 2, 0));
}

#[tokio::test]
async fn repeated_invocations_and_guest_ads_with_clock_lag_cannot_exhaust_bot_screening() {
    let (mut e, _) = setup(Some(0.07));
    let output = support::Capture::default();
    e.logger = Arc::new(Logger::with_writer(LogLevel::Verbose, &[], output.clone()));
    let mut settings = e.settings(CHAT).unwrap();
    settings.screening = ScreeningMode::FirstSeen;
    e.store.put(CHAT, "settings", &settings).unwrap();
    for id in [1, 3] {
        let mut invocation = message(id);
        invocation["date"] = json!(NOW + 299);
        invocation["text"] = json!("@guest_bot");
        e.message(CHAT, &invocation).unwrap();
        drain(&e).await;
        *e.services.probability.lock().unwrap() = Some(0.96);
        let mut reply = guest(id + 1, USER);
        reply["date"] = json!(NOW + 299);
        reply["text"] = json!("Synthetic promotion");
        reply["reply_markup"] =
            json!({"inline_keyboard":[[{"text":"Offer","url":"https://example.invalid/offer"}]]});
        e.message(CHAT, &reply).unwrap();
        e.message(CHAT, &reply).unwrap();
        drain(&e).await;
    }
    let all = cases(&e);
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter().find(|c| c.message == 1).unwrap().state,
        "allowed"
    );
    for id in [2, 4] {
        let c = all.iter().find(|c| c.message == id).unwrap();
        assert_eq!(c.kind, CaseKind::Guest);
        assert_eq!(c.state, "banned");
        assert!(c.deleted && c.banned);
        let caller = c.guest_caller.as_ref().unwrap();
        assert_eq!(caller.user, USER);
        assert_eq!(caller.message, Some(id - 1));
        assert!(caller.deleted && caller.banned);
    }
    assert_eq!(e.services.evidence.lock().unwrap().len(), 3);
    assert_eq!(bans(&e), vec![GUEST, USER, GUEST, USER]);
    assert_eq!(
        e.services
            .calls("deleteMessage")
            .iter()
            .map(|r| r["message_id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 1, 4, 3]
    );
    let skips: Vec<_> = output
        .records()
        .into_iter()
        .filter(|r| r["event"] == "screening.skip")
        .collect();
    assert_eq!(skips.len(), 3);
    assert_eq!(
        skips[0]["fields"]["reason"],
        "guest_message_already_screened"
    );
    assert_eq!(skips[1]["fields"]["reason"], "not_first_message");
    assert_eq!(
        skips[2]["fields"]["reason"],
        "guest_message_already_screened"
    );
}

#[tokio::test]
async fn guest_partial_failure_retries_only_unfinished_actions() {
    let (e, time) = setup(Some(0.99));
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("banChatMember".into(), 1);
    e.message(CHAT, &guest(2, USER)).unwrap();
    drain(&e).await;
    let c = cases(&e).pop().unwrap();
    assert!(!c.banned);
    assert!(c.guest_caller.unwrap().banned);
    assert_eq!(bans(&e), vec![GUEST, USER]);
    time.fetch_add(10, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(bans(&e), vec![GUEST, USER, GUEST]);
    assert_eq!(e.services.calls("deleteMessage").len(), 2);
    assert_eq!(cases(&e)[0].state, "banned");
}

#[tokio::test]
async fn guest_admin_or_rejoined_caller_is_protected_independently() {
    for mode in ["promoted", "rejoined", "same_second"] {
        let (e, time) = setup(Some(0.99));
        e.joined(CHAT, USER, NOW, true).unwrap();
        e.message(CHAT, &guest(2, USER)).unwrap();
        let job = e.store.claim(NOW).unwrap().unwrap();
        assert_eq!(job.kind, "classify");
        e.run(&job).await.unwrap();
        if mode == "promoted" {
            e.services.member(USER, json!({"status":"administrator"}));
        } else if mode == "same_second" {
            e.message(CHAT, &json!({"date":NOW,"left_chat_member":{"id":USER}}))
                .unwrap();
            e.joined(CHAT, USER, NOW, true).unwrap();
        } else {
            time.fetch_add(1, Ordering::SeqCst);
            e.joined(CHAT, USER, e.now(), true).unwrap();
        }
        drain(&e).await;
        assert_eq!(bans(&e), vec![GUEST]);
        assert_eq!(e.services.calls("deleteMessage").len(), 1);
        assert!(cases(&e)[0].guest_caller.as_ref().unwrap().protected);
    }
}

#[tokio::test]
async fn guest_channel_identity_and_unrelated_replies_never_infer_a_user() {
    let (e, _) = setup(Some(0.99));
    let mut m = guest(2, USER);
    m["guest_bot_caller_user"] = Value::Null;
    m["guest_bot_caller_chat"] = json!({"id":-100555,"type":"channel","title":"频道"});
    e.message(CHAT, &m).unwrap();
    drain(&e).await;
    assert_eq!(bans(&e), vec![GUEST]);
    assert!(cases(&e)[0].guest_caller.is_none());
    assert_eq!(e.services.calls("deleteMessage").len(), 1);
    let mut m = guest(4, 43);
    m["reply_to_message"]["chat"]["id"] = json!(-100555);
    e.message(CHAT, &m).unwrap();
    drain(&e).await;
    assert!(
        cases(&e)
            .iter()
            .find(|c| c.message == 4)
            .unwrap()
            .guest_caller
            .as_ref()
            .unwrap()
            .message
            .is_none()
    );
    assert_eq!(bans(&e), vec![GUEST, GUEST, 43]);
    assert_eq!(e.services.calls("deleteMessage").len(), 2);
}

#[tokio::test]
async fn guest_moderation_honors_manual_review_and_unban_cancels_both_targets() {
    let (e, _) = setup(Some(0.7));
    e.services.member(7, json!({"status":"creator"}));
    e.message(CHAT, &guest(2, USER)).unwrap();
    drain(&e).await;
    assert!(bans(&e).is_empty());
    let c = cases(&e).pop().unwrap();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "ban", &c.id))
        .await
        .unwrap();
    admin::handle(
        &e,
        CHAT,
        &admin_message(7, &format!("/unban {CHAT} {USER}")),
    )
    .await
    .unwrap();
    drain(&e).await;
    assert!(bans(&e).is_empty());
    e.message(CHAT, &guest(4, 43)).unwrap();
    drain(&e).await;
    let c = cases(&e).into_iter().find(|c| c.message == 4).unwrap();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "ban", &c.id))
        .await
        .unwrap();
    drain(&e).await;
    assert_eq!(bans(&e), vec![GUEST, 43]);
}

#[tokio::test]
async fn guest_query_business_or_unmanaged_contexts_cannot_trigger_moderation() {
    let (e, _) = setup(Some(0.99));
    for field in ["guest_query_id", "business_connection_id"] {
        let mut m = guest(2, USER);
        m[field] = json!("different-context");
        e.message(CHAT, &m).unwrap();
    }
    e.message(CHAT - 1, &guest(2, USER)).unwrap();
    drain(&e).await;
    assert!(cases(&e).is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
    let mut s = e.settings(CHAT).unwrap();
    s.spam = false;
    e.store.put(CHAT, "settings", &s).unwrap();
    e.message(CHAT, &guest(2, USER)).unwrap();
    drain(&e).await;
    assert!(cases(&e).is_empty());
}

#[tokio::test]
async fn join_profile_toggle_is_per_group_authorized_and_persistent() {
    let (e, _) = setup(Some(0.1));
    admin::handle(&e, CHAT, &admin_callback(USER, CHAT, "profileon", ""))
        .await
        .unwrap();
    assert!(!e.settings(CHAT).unwrap().join_profile_review);
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "profileon", ""))
        .await
        .unwrap();
    assert!(e.settings(CHAT).unwrap().join_profile_review);
    assert!(!e.settings(CHAT - 1).unwrap().join_profile_review);
    assert!(text(&e).contains("入群资料审核：开启"));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "profileoff", ""))
        .await
        .unwrap();
    assert!(!e.settings(CHAT).unwrap().join_profile_review);
}

#[tokio::test]
async fn optional_join_profile_requires_captcha_and_profile_in_either_completion_order() {
    for captcha_first in [true, false] {
        let (e, time) = setup(if captcha_first { None } else { Some(0.1) });
        enable_join_profiles(&e);
        let mut r = join(NOW);
        r["bio"] = json!("申请资料 @old_name");
        profile(&e, USER, "当前资料 @new_name");
        e.join_request(CHAT, &r, 1).unwrap();
        drain(&e).await;
        assert!(e.services.calls("approveChatJoinRequest").is_empty());
        e.receive_verification(&submission(&e, &session(&e), USER))
            .unwrap();
        drain(&e).await;
        if captcha_first {
            assert!(e.services.calls("approveChatJoinRequest").is_empty());
            *e.services.probability.lock().unwrap() = Some(0.1);
            time.fetch_add(10, Ordering::SeqCst);
            drain(&e).await;
        }
        assert_eq!(session(&e).state, "approved");
        assert_eq!(e.services.calls("approveChatJoinRequest").len(), 1);
        let sent = e.services.evidence.lock().unwrap()[0].clone();
        assert_eq!(sent["context"], "join_profile");
        assert_eq!(
            sent["join_request_profile"]["bio"]["mentions"],
            json!(["@old_name"])
        );
        assert_eq!(
            sent["profiles"]["user"]["bio"]["mentions"],
            json!(["@new_name"])
        );
        let stats = e
            .store
            .statistics_report(CHAT, day(NOW), day(NOW), e.now())
            .unwrap();
        assert_eq!((stats.messages, stats.profiles, stats.approved), (0, 1, 1));
    }
}

#[tokio::test]
async fn join_profile_manual_allow_still_requires_captcha_and_ban_closes_approval_gate() {
    for ban in [true, false] {
        let (e, _) = setup(Some(0.799999));
        enable_join_profiles(&e);
        e.join_request(CHAT, &join(NOW), 1).unwrap();
        drain(&e).await;
        assert_eq!(cases(&e)[0].state, "review");
        assert!(bans(&e).is_empty());
        let id = cases(&e)[0].id.clone();
        assert!(admin::decide(&e, CHAT, &id, ban, 7).unwrap());
        drain(&e).await;
        assert!(e.services.calls("approveChatJoinRequest").is_empty());
        e.receive_verification(&submission(&e, &session(&e), USER))
            .unwrap();
        drain(&e).await;
        if ban {
            assert_eq!(session(&e).state, "rejected");
            assert_eq!(bans(&e), vec![USER]);
            assert!(e.services.calls("approveChatJoinRequest").is_empty());
        } else {
            assert_eq!(session(&e).state, "approved");
        }
        assert!(e.services.calls("deleteMessage").is_empty());
    }
}

#[tokio::test]
async fn high_risk_join_profile_bans_while_replaced_or_expired_request_cannot_punish() {
    let (e, _) = setup(Some(0.8));
    enable_join_profiles(&e);
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    drain(&e).await;
    assert_eq!(bans(&e), vec![USER]);
    assert_eq!(session(&e).state, "rejected");
    for replace in [true, false] {
        let (e, time) = setup(Some(0.99));
        enable_join_profiles(&e);
        e.join_request(CHAT, &join(NOW), 1).unwrap();
        let old = session(&e);
        let case = old.profile_case.clone().unwrap();
        if replace {
            e.join_request(CHAT, &join(NOW + 1), 2).unwrap();
        } else {
            time.fetch_add(601, Ordering::SeqCst);
        }
        let job = fuuki_iin_bot::model::Job {
            id: "direct-old-case".into(),
            chat: CHAT,
            kind: "classify".into(),
            payload: json!({"id":case}),
            attempts: 1,
            created_at: NOW,
        };
        e.execute(&job).await.unwrap();
        assert!(e.services.evidence.lock().unwrap().is_empty());
        assert!(bans(&e).is_empty());
        assert!(!admin::decide(&e, CHAT, &case, true, 7).unwrap());
    }
}

#[tokio::test]
async fn profile_screening_disabled_does_not_skip_message_profile_and_captcha_disabled_does_not_auto_admit()
 {
    let (e, _) = setup(Some(0.1));
    enable_join_profiles(&e);
    let mut s = e.settings(CHAT).unwrap();
    s.captcha = false;
    e.store.put(CHAT, "settings", &s).unwrap();
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    drain(&e).await;
    assert_eq!(session(&e).state, "manual");
    assert_eq!(cases(&e)[0].state, "allowed");
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    s.join_profile_review = false;
    s.screening = ScreeningMode::FirstSeen;
    e.store.put(CHAT, "settings", &s).unwrap();
    profile(&e, USER, "message-time @profile");
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    assert_eq!(
        e.services.evidence.lock().unwrap()[1]["profiles"]["user"]["bio"]["mentions"],
        json!(["@profile"])
    );
    assert!(cases(&e).iter().any(|c| c.kind == CaseKind::Message));
}

#[tokio::test]
async fn profile_and_guest_evidence_is_visible_in_authorized_audit_snapshot() {
    let (e, _) = setup(Some(0.7));
    profile(
        &e,
        USER,
        &format!("t.me/audit_bio @audit_contact {}", "证".repeat(3500)),
    );
    e.services.member(7, json!({"status":"creator"}));
    e.message(CHAT, &guest(2, USER)).unwrap();
    drain(&e).await;
    let page = e
        .store
        .audit_page(CHAT, fuuki_iin_bot::audit::Cursor::default())
        .unwrap();
    let entry = page
        .entries
        .iter()
        .find(|r| r.action == "classified")
        .unwrap();
    e.services.calls.lock().unwrap().clear();
    admin::handle(
        &e,
        CHAT,
        &admin_callback(7, CHAT, "auditentry", &entry.id.to_string()),
    )
    .await
    .unwrap();
    for _ in 0..100 {
        let reply = e.services.replies().last().unwrap().clone();
        let next = reply["reply_markup"]["inline_keyboard"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap())
            .find(|button| button["text"] == "下一页")
            .map(|button| button["callback_data"].as_str().unwrap().to_owned());
        let Some(next) = next else { break };
        let mut parts = next.split('|');
        let action = parts.next().unwrap();
        let chat = parts.next().unwrap().parse().unwrap();
        let cursor = parts.next().unwrap();
        admin::handle(&e, chat, &admin_callback(7, chat, action, cursor))
            .await
            .unwrap();
    }
    assert!(e.services.calls("sendMessage").is_empty());
    assert!(e.services.calls("editMessageText").len() > 1);
    assert!(
        e.services
            .calls("editMessageText")
            .iter()
            .all(|reply| { reply["chat_id"] == 7 && reply["message_id"] == 777 })
    );
    let output = text(&e);
    for expected in [
        "Guest Bot 召唤者",
        "t.me/audit_bio",
        "@audit_contact",
        "guest_bot",
        "一天八万",
        "https://t.me/spam_channel",
        "💰",
    ] {
        assert!(output.contains(expected), "missing {expected}");
    }
    assert_eq!(output.matches('证').count(), 3500);
    assert!(!output.contains("must-not-leak"));
    assert!(
        e.services
            .replies()
            .iter()
            .all(|m| formatting::rendered(m).encode_utf16().count() <= 4096)
    );
}

#[tokio::test]
async fn exhausted_join_profile_needs_human_review_and_invite_permission() {
    let (e, time) = setup(None);
    enable_join_profiles(&e);
    e.join_request(CHAT, &join(NOW), 1).unwrap();
    for _ in 0..3 {
        drain(&e).await;
        time.fetch_add(60, Ordering::SeqCst);
    }
    let c = cases(&e).pop().unwrap();
    assert_eq!(c.state, "review");
    assert!(c.reason.is_some());
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    drain(&e).await;
    assert_eq!(session(&e).state, "verified");
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    e.services.member(7,json!({"status":"administrator","can_delete_messages":true,"can_restrict_members":true,"can_invite_users":false}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "allow", &c.id))
        .await
        .unwrap();
    assert_eq!(cases(&e)[0].state, "review");
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "allow", &c.id))
        .await
        .unwrap();
    drain(&e).await;
    assert_eq!(session(&e).state, "approved");
    let r = e
        .store
        .statistics_report(CHAT, day(NOW), day(NOW), e.now())
        .unwrap();
    assert_eq!(
        (r.messages, r.profiles, r.profile_failed, r.attempts),
        (0, 1, 1, 3)
    );
}

#[tokio::test]
async fn guest_retry_progress_survives_database_reopen_and_schema_two_stats_migrate() {
    let (mut e, time) = setup(Some(0.99));
    let dir =
        std::env::temp_dir().join(format!("fuuki-iin-guest-restart-{}", uuid::Uuid::new_v4()));
    let path = dir.join("bot.sqlite");
    e.store = Arc::new(Store::open(&path).unwrap());
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("banChatMember".into(), 1);
    e.message(CHAT, &guest(2, USER)).unwrap();
    drain(&e).await;
    let mut legacy = serde_json::to_value(&cases(&e)[0]).unwrap();
    for key in ["kind", "guest_caller", "join_session"] {
        legacy.as_object_mut().unwrap().remove(key);
    }
    let legacy: Case = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.kind, CaseKind::Message);
    e.store = Arc::new(Store::memory().unwrap());
    // A schema 2 statistics table contains the same message facts without the new kind column.
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("ALTER TABLE statistics_cases DROP COLUMN kind; PRAGMA user_version=2;")
            .unwrap();
    }
    e.store = Arc::new(Store::open(&path).unwrap());
    e.store.recover(e.now()).unwrap();
    time.fetch_add(10, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(bans(&e), vec![GUEST, USER, GUEST]);
    assert_eq!(e.services.calls("deleteMessage").len(), 2);
    let r = e
        .store
        .statistics_report(CHAT, day(NOW), day(NOW), e.now())
        .unwrap();
    assert_eq!((r.messages, r.spam, r.profiles), (1, 1, 0));
    e.store = Arc::new(Store::memory().unwrap());
    e.store = Arc::new(Store::open(&path).unwrap());
    assert_eq!(cases(&e)[0].state, "banned");
    drop(e);
    std::fs::remove_dir_all(dir).unwrap();
}
