use super::*;
use fuuki_iin_bot::{
    audit::{Cursor, Filter},
    chat_info,
};

fn last_reply(e: &Engine<Mock>) -> Value {
    e.services.calls("sendMessage").last().unwrap().clone()
}
fn callback_data(reply: &Value, label: &str) -> String {
    reply["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r.as_array().unwrap())
        .find(|b| b["text"] == label)
        .unwrap()["callback_data"]
        .as_str()
        .unwrap()
        .into()
}
async fn click(e: &Engine<Mock>, data: &str) {
    let parts = data.split('|').collect::<Vec<_>>();
    let chat = parts[1].parse().unwrap();
    admin::handle(e, chat, &admin_callback(7, chat, parts[0], parts[2]))
        .await
        .unwrap();
}
fn texts(e: &Engine<Mock>) -> String {
    e.services
        .calls("sendMessage")
        .iter()
        .map(super::formatting::rendered)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn group_titles_refresh_with_fallback_but_never_cache_authorization() {
    let (e, time) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    e.services.chats.lock().unwrap().insert(
        CHAT,
        json!({"id":CHAT,"type":"supergroup","title":"开发者讨论群"}),
    );
    admin::handle(&e, 0, &admin_message(7, "/admin"))
        .await
        .unwrap();
    assert_eq!(
        last_reply(&e)["reply_markup"]["inline_keyboard"][0][0]["text"],
        format!("开发者讨论群 · {CHAT}")
    );
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "panel", ""))
        .await
        .unwrap();
    assert!(texts(&e).contains("开发者讨论群"));
    assert_eq!(e.services.calls("getChat").len(), 1);
    assert_eq!(e.services.calls("getChatMember").len(), 2);
    time.fetch_add(300, Ordering::SeqCst);
    e.services.chats.lock().unwrap().insert(
        CHAT,
        json!({"id":CHAT,"type":"supergroup","title":"新群名"}),
    );
    assert!(chat_info::label(&e, CHAT).await.unwrap().contains("新群名"));
    time.fetch_add(300, Ordering::SeqCst);
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChat".into(), 1);
    assert!(chat_info::label(&e, CHAT).await.unwrap().contains("新群名"));
    e.services.calls.lock().unwrap().clear();
    e.services.member(7, json!({"status":"member"}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "panel", ""))
        .await
        .unwrap();
    assert!(e.services.calls("sendMessage").is_empty());
    assert_eq!(e.services.calls("answerCallbackQuery")[0]["text"], "不可用");
    assert!(e.services.calls("getChat").is_empty());
}

#[tokio::test]
async fn title_lookup_failure_keeps_admin_usable_and_ignores_unmanaged_groups() {
    let (e, _) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChat".into(), 1);
    admin::handle(&e, 0, &admin_message(7, "/admin"))
        .await
        .unwrap();
    assert!(
        last_reply(&e)["reply_markup"]["inline_keyboard"][0][0]["text"]
            .as_str()
            .unwrap()
            .contains(&CHAT.to_string())
    );
    let before = e.services.calls("getChat").len();
    chat_info::label(&e, CHAT - 1).await.unwrap();
    assert_eq!(e.services.calls("getChat").len(), before);
}

#[test]
fn audit_pages_filter_and_remain_stable_as_new_events_arrive() {
    let (e, _) = setup(None);
    for n in 0..12 {
        e.audit(CHAT, "classified", &format!("case{n}"), "p=0.2", None)
            .unwrap();
        e.audit(CHAT - 1, "classified", "other-group", "", None)
            .unwrap();
    }
    e.audit(CHAT, "settings_changed", "settings", "spam=false", Some(7))
        .unwrap();
    e.audit(CHAT, "captcha_verified", "42", "", None).unwrap();
    e.audit(CHAT, "review_allow", "case0", "", Some(7)).unwrap();
    e.audit(CHAT, "job_failed", "job1", "timeout", None)
        .unwrap();
    let first = e
        .store
        .audit_page(
            CHAT,
            Cursor {
                filter: Filter::Classification,
                ..Cursor::default()
            },
        )
        .unwrap();
    assert_eq!((first.total, first.pages, first.entries.len()), (12, 3, 5));
    assert_eq!(first.entries[0].target, "case11");
    assert!(first.entries[0].time.starts_with("2026-"));
    e.audit(CHAT, "classified", "new-event", "", None).unwrap();
    let second = e
        .store
        .audit_page(
            CHAT,
            Cursor {
                page: 1,
                ..first.cursor
            },
        )
        .unwrap();
    assert_eq!(
        (second.total, second.entries[0].target.as_str()),
        (12, "case6")
    );
    let third = e
        .store
        .audit_page(
            CHAT,
            Cursor {
                page: 999_999,
                ..first.cursor
            },
        )
        .unwrap();
    assert_eq!((third.cursor.page, third.entries.len()), (2, 2));
    let previous = e
        .store
        .audit_page(
            CHAT,
            Cursor {
                page: 0,
                ..second.cursor
            },
        )
        .unwrap();
    assert_eq!(
        previous.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
        first.entries.iter().map(|e| e.id).collect::<Vec<_>>()
    );
    assert_eq!(
        e.store
            .audit_page(
                CHAT,
                Cursor {
                    filter: Filter::Classification,
                    ..Cursor::default()
                }
            )
            .unwrap()
            .total,
        13
    );
    for (filter, action) in [
        (Filter::Moderation, "review_allow"),
        (Filter::Joins, "captcha_verified"),
        (Filter::Settings, "settings_changed"),
        (Filter::Errors, "job_failed"),
    ] {
        let page = e
            .store
            .audit_page(
                CHAT,
                Cursor {
                    filter,
                    ..Cursor::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.entries[0].action, action);
    }
    let empty = e.store.audit_page(CHAT - 2, Cursor::default()).unwrap();
    assert_eq!((empty.total, empty.pages, empty.cursor.page), (0, 1, 0));
}

#[tokio::test]
async fn audit_buttons_navigate_filter_refresh_and_reject_bad_cursors() {
    let (e, _) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    for n in 0..11 {
        e.audit(CHAT, "classified", &n.to_string(), "p=0.1", None)
            .unwrap();
    }
    admin::handle(&e, CHAT, &admin_message(7, &format!("/audit {CHAT}")))
        .await
        .unwrap();
    assert!(texts(&e).contains("第 1/3 页"));
    let first = last_reply(&e);
    click(&e, &callback_data(&first, "下一页")).await;
    assert!(
        last_reply(&e)["text"]
            .as_str()
            .unwrap()
            .contains("第 2/3 页")
    );
    click(&e, &callback_data(&last_reply(&e), "上一页")).await;
    assert_eq!(last_reply(&e)["text"], first["text"]);
    e.audit(
        CHAT,
        "settings_changed",
        "group",
        "settings-detail",
        Some(7),
    )
    .unwrap();
    click(&e, &callback_data(&last_reply(&e), "设置变更")).await;
    assert!(
        last_reply(&e)["text"]
            .as_str()
            .unwrap()
            .contains("暂无此类型")
    );
    click(&e, &callback_data(&last_reply(&e), "刷新")).await;
    assert!(
        last_reply(&e)["text"]
            .as_str()
            .unwrap()
            .contains("settings-detail")
    );
    assert!(!last_reply(&e)["text"].as_str().unwrap().contains("p=0.1"));
    for invalid in [
        "bad",
        "x:1:0",
        "a:-1:0",
        "c:n:1",
        "a:1:1000001",
        "a:1:0:extra",
        "a:9223372036854775808:0",
    ] {
        admin::handle(&e, CHAT, &admin_callback(7, CHAT, "audit", invalid))
            .await
            .unwrap();
        assert!(
            last_reply(&e)["text"]
                .as_str()
                .unwrap()
                .contains("分页参数无效")
        );
    }
    for call in e.services.calls("sendMessage") {
        assert!(call["text"].as_str().unwrap().encode_utf16().count() <= 4096);
        if let Some(rows) = call["reply_markup"]["inline_keyboard"].as_array() {
            for b in rows.iter().flat_map(|r| r.as_array().unwrap()) {
                assert!(b["callback_data"].as_str().unwrap().len() <= 64);
                assert!(!b["text"].as_str().unwrap().contains(" / "));
            }
        }
    }
    let worst = Cursor {
        filter: Filter::All,
        snapshot: Some(i64::MAX),
        page: 1_000_000,
    };
    assert!(format!("audit|{}|{}", i64::MIN, worst.encode()).len() <= 64);
    assert_chinese_admin_replies(&e);
}

#[tokio::test]
async fn audit_retains_deleted_message_body_inline_bot_buttons_and_handles_expiry() {
    let (e, time) = setup(Some(0.99));
    e.services.member(7, json!({"status":"creator"}));
    e.joined(CHAT, USER, NOW, true).unwrap();
    let mut m = message(81);
    m["text"] = json!(format!("广告正文{}结尾", "💰".repeat(2000)));
    m["via_bot"] = json!({"id":91,"is_bot":true,"username":"sample_bot","first_name":"示例机器人"});
    m["reply_markup"] = json!({"inline_keyboard":[[{"text":"发财入口","url":"https://example.invalid/promotion"}]]});
    m["entities"] =
        json!([{"type":"text_link","offset":0,"length":1,"url":"https://example.invalid/hidden"}]);
    e.message(CHAT, &m).unwrap();
    drain(&e).await;
    assert!(cases(&e)[0].deleted);
    let page = e
        .store
        .audit_page(
            CHAT,
            Cursor {
                filter: Filter::Classification,
                ..Cursor::default()
            },
        )
        .unwrap();
    let id = page.entries[0].id.to_string();
    e.services.calls.lock().unwrap().clear();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "auditentry", &id))
        .await
        .unwrap();
    let text = texts(&e);
    for part in [
        "原消息快照",
        "新成员",
        "new_user",
        "sample_bot",
        "广告正文",
        "结尾",
        "发财入口",
        "https://example.invalid/promotion",
        "https://example.invalid/hidden",
        "99.0%",
    ] {
        assert!(text.contains(part), "missing {part}");
    }
    assert_eq!(text.matches('💰').count(), 2000);
    for call in e.services.calls("sendMessage") {
        assert_eq!(call["chat_id"], 7);
        assert_eq!(call["parse_mode"], "MarkdownV2");
        assert!(call["text"].as_str().unwrap().encode_utf16().count() <= 4096);
        assert!(!call["reply_markup"].to_string().contains("example.invalid"));
    }
    time.fetch_add(604_801, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    e.services.calls.lock().unwrap().clear();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "auditentry", &id))
        .await
        .unwrap();
    assert!(texts(&e).contains("按保留策略清理"));
    assert!(!texts(&e).contains("广告正文"));
    time.fetch_add(2_592_001, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    e.services.calls.lock().unwrap().clear();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "auditentry", &id))
        .await
        .unwrap();
    assert!(texts(&e).contains("记录不存在或已过保留期限"));
}

#[tokio::test]
async fn audit_details_require_current_admin_and_config_and_never_cross_groups() {
    let (mut e, _) = setup(Some(0.1));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let entry = e
        .store
        .audit_page(
            CHAT,
            Cursor {
                filter: Filter::Classification,
                ..Cursor::default()
            },
        )
        .unwrap()
        .entries
        .remove(0);
    let id = entry.id.to_string();
    let config = Arc::get_mut(&mut e.config).unwrap();
    config.super_admins = vec![7];
    config.chats.push(CHAT - 1);
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "auditentry", &id))
        .await
        .unwrap();
    assert!(texts(&e).contains("Hello"));
    e.services.calls.lock().unwrap().clear();
    // Even a super admin cannot read group A's event using group B's scope.
    admin::handle(
        &e,
        CHAT - 1,
        &admin_callback(7, CHAT - 1, "auditentry", &id),
    )
    .await
    .unwrap();
    assert!(!texts(&e).contains("Hello"));
    assert!(texts(&e).contains("记录不存在"));
    for action in ["audit", "auditentry"] {
        e.services.calls.lock().unwrap().clear();
        admin::handle(&e, CHAT, &admin_callback(8, CHAT, action, &id))
            .await
            .unwrap();
        assert!(e.services.calls("sendMessage").is_empty());
        assert!(e.services.calls("getChat").is_empty());
    }
    Arc::get_mut(&mut e.config)
        .unwrap()
        .chats
        .retain(|c| *c != CHAT);
    e.services.calls.lock().unwrap().clear();
    for action in ["audit", "auditentry"] {
        admin::handle(&e, CHAT, &admin_callback(7, CHAT, action, &id))
            .await
            .unwrap();
    }
    assert!(e.services.calls.lock().unwrap().is_empty());
}

#[test]
fn expired_audit_ids_are_never_reused_by_new_events() {
    let dir = std::env::temp_dir().join(format!("fuuki-iin-audit-{}", uuid::Uuid::new_v4()));
    let path = dir.join("bot.sqlite");
    drop(Store::open(&path).unwrap());
    {
        // A pre-upgrade database has existing events but no sequence metadata.
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("DELETE FROM meta WHERE key='audit_sequence'", [])
            .unwrap();
        db.execute("INSERT INTO audit(id,chat,at,action,target,detail) VALUES (81,?1,?2,'classified','old-case','')", rusqlite::params![CHAT, NOW]).unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.audit_page(CHAT, Cursor::default()).unwrap().entries[0].id,
            81
        );
        // Retention can run before the first new audit write after an upgrade.
        store.prune(NOW + 2_592_001).unwrap();
    }
    let next;
    {
        let store = Store::open(&path).unwrap();
        store
            .audit(CHAT, NOW, "classified", None, "new-case", "")
            .unwrap();
        assert!(store.audit_entry(CHAT, 81).unwrap().is_none());
        next = store.audit_page(CHAT, Cursor::default()).unwrap().entries[0].id;
        assert!(next > 81);
        store.prune(NOW + 2_592_001).unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        store
            .audit(CHAT, NOW, "classified", None, "later-case", "")
            .unwrap();
        assert!(store.audit_entry(CHAT, next).unwrap().is_none());
        assert!(store.audit_page(CHAT, Cursor::default()).unwrap().entries[0].id > next);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn existing_admin_command_scopes_migrate_to_chinese_without_public_grants() {
    let (e, _) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    e.store
        .put(0, "menu:7", &json!({"user":7,"applied":true,"version":1}))
        .unwrap();
    menus::refresh(&e).await.unwrap();
    let commands = e.services.calls("setMyCommands");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0]["scope"], json!({"type":"chat","chat_id":7}));
    for command in commands[0]["commands"].as_array().unwrap() {
        assert!(!command["description"].as_str().unwrap().contains(" / "));
    }
    menus::refresh(&e).await.unwrap();
    assert_eq!(e.services.calls("setMyCommands").len(), 1);
}
