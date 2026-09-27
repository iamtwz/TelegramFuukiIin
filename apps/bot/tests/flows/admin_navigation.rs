use super::*;

const ADMIN: i64 = 7;
const MENU_MESSAGE: i64 = 777;

fn last_reply(e: &Engine<Mock>) -> Value {
    e.services.replies().last().unwrap().clone()
}

fn buttons(reply: &Value) -> Vec<Value> {
    reply["reply_markup"]["inline_keyboard"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .flat_map(|row| row.as_array().unwrap())
                .filter(|button| button["callback_data"].is_string())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn callback(reply: &Value, action: &str) -> String {
    buttons(reply)
        .into_iter()
        .find_map(|button| {
            let data = button["callback_data"].as_str()?;
            (data.split('|').next() == Some(action)).then(|| data.to_owned())
        })
        .unwrap_or_else(|| panic!("missing {action} button: {reply}"))
}

fn next_page(reply: &Value) -> Option<String> {
    buttons(reply).into_iter().find_map(|button| {
        (button["text"] == "下一页").then(|| button["callback_data"].as_str().unwrap().to_owned())
    })
}

async fn click(e: &Engine<Mock>, data: &str) {
    let mut parts = data.split('|');
    let action = parts.next().unwrap();
    let chat = parts.next().unwrap().parse().unwrap();
    let id = parts.next().unwrap();
    assert!(parts.next().is_none());
    let sends = e.services.calls("sendMessage").len();
    let edits = e.services.calls("editMessageText").len();
    admin::handle(e, chat, &admin_callback(ADMIN, chat, action, id))
        .await
        .unwrap();
    assert_eq!(e.services.calls("sendMessage").len(), sends);
    assert_eq!(e.services.calls("editMessageText").len(), edits + 1);
    let reply = last_reply(e);
    assert_eq!(reply["chat_id"], ADMIN);
    assert_eq!(reply["message_id"], MENU_MESSAGE);
    assert!(formatting::rendered(&reply).encode_utf16().count() <= 4096);
    for button in buttons(&reply) {
        assert!(button["callback_data"].as_str().unwrap().len() <= 64);
    }
}

async fn review_cases(e: &Engine<Mock>, count: i64) {
    for n in 0..count {
        let user = USER + n;
        e.joined(CHAT, user, NOW, true).unwrap();
        let mut m = message(200 + n);
        m["from"]["id"] = json!(user);
        e.message(CHAT, &m).unwrap();
        drain(e).await;
    }
    assert_eq!(cases(e).len(), count as usize);
    assert!(cases(e).iter().all(|case| case.state == "review"));
    e.services.calls.lock().unwrap().clear();
}

async fn detail_pages(e: &Engine<Mock>) -> Vec<String> {
    let mut pages = vec![];
    for _ in 0..100 {
        let reply = last_reply(e);
        pages.push(formatting::rendered(&reply));
        let Some(next) = next_page(&reply) else {
            return pages;
        };
        click(e, &next).await;
    }
    panic!("detail navigation did not finish");
}

#[tokio::test]
async fn admin_navigation_updates_one_message_and_returns_to_parent_menus() {
    let (e, _) = setup(Some(0.75));
    review_cases(&e, 6).await;
    e.services.member(ADMIN, json!({"status":"creator"}));
    admin::handle(&e, 0, &admin_message(ADMIN, "/admin"))
        .await
        .unwrap();
    assert_eq!(e.services.calls("sendMessage").len(), 1);
    assert!(e.services.calls("editMessageText").is_empty());
    click(&e, &callback(&last_reply(&e), "panel")).await;
    assert_eq!(callback(&last_reply(&e), "groups"), "groups|0|0");
    click(&e, &callback(&last_reply(&e), "spamoff")).await;
    assert!(!e.settings(CHAT).unwrap().spam);
    click(&e, &callback(&last_reply(&e), "spamon")).await;
    assert!(e.settings(CHAT).unwrap().spam);
    click(&e, &callback(&last_reply(&e), "pending")).await;
    assert_eq!(
        buttons(&last_reply(&e))
            .iter()
            .filter(|button| button["callback_data"]
                .as_str()
                .unwrap()
                .starts_with("case|"))
            .count(),
        5
    );
    let first_list = last_reply(&e);
    click(&e, &next_page(&first_list).unwrap()).await;
    assert_eq!(
        buttons(&last_reply(&e))
            .iter()
            .filter(|button| button["callback_data"]
                .as_str()
                .unwrap()
                .starts_with("case|"))
            .count(),
        1
    );
    let second_list = last_reply(&e);
    let second_offset = next_page(&first_list)
        .unwrap()
        .split('|')
        .nth(2)
        .unwrap()
        .to_owned();
    click(&e, &callback(&second_list, "case")).await;
    assert_eq!(
        callback(&last_reply(&e), "pending"),
        format!("pending|{CHAT}|{second_offset}")
    );
    click(&e, &callback(&last_reply(&e), "pending")).await;
    assert_eq!(
        formatting::rendered(&last_reply(&e)),
        formatting::rendered(&second_list)
    );
    click(&e, &callback(&last_reply(&e), "panel")).await;
    for action in ["health", "stats", "audit"] {
        click(&e, &callback(&last_reply(&e), action)).await;
        assert_eq!(callback(&last_reply(&e), "panel"), format!("panel|{CHAT}|"));
        click(&e, &callback(&last_reply(&e), "panel")).await;
    }
    click(&e, &callback(&last_reply(&e), "groups")).await;
    assert!(
        last_reply(&e)["reply_markup"]
            .to_string()
            .contains(&format!("panel|{CHAT}|"))
    );
    assert_eq!(e.services.calls("sendMessage").len(), 1);
    assert_chinese_admin_replies(&e);
}

#[tokio::test]
async fn long_case_evidence_is_complete_across_pages_of_the_same_message() {
    let (e, _) = setup(Some(0.75));
    review_cases(&e, 1).await;
    let mut case = cases(&e).remove(0);
    case.evidence = Some(json!({"text":format!("原文起点{}原文终点", "💰界".repeat(3000))}));
    e.store
        .put(CHAT, &format!("case:{}", case.id), &case)
        .unwrap();
    e.services.member(ADMIN, json!({"status":"creator"}));
    admin::handle(
        &e,
        CHAT,
        &admin_message(ADMIN, &format!("/case {CHAT} {}", case.id)),
    )
    .await
    .unwrap();
    assert_eq!(e.services.calls("sendMessage").len(), 1);
    let first = last_reply(&e);
    let pages = detail_pages(&e).await;
    assert!(pages.len() > 1);
    let contents = pages.join("\n");
    assert!(contents.contains("原文起点"));
    assert!(contents.contains("原文终点"));
    assert_eq!(contents.matches('💰').count(), 3000);
    assert_eq!(contents.matches('界').count(), 3000);
    assert_eq!(
        callback(&last_reply(&e), "pending"),
        format!("pending|{CHAT}|0")
    );
    let previous = buttons(&last_reply(&e))
        .into_iter()
        .find(|button| button["text"] == "上一页")
        .unwrap()["callback_data"]
        .as_str()
        .unwrap()
        .to_owned();
    click(&e, &previous).await;
    assert!(next_page(&last_reply(&e)).is_some());
    assert!(callback(&first, "allow").ends_with(&case.id));
    assert!(callback(&first, "ban").ends_with(&case.id));
    assert_eq!(e.services.calls("sendMessage").len(), 1);
}

#[tokio::test]
async fn empty_lists_keep_navigation_and_large_group_lists_page_in_place() {
    let (mut e, _) = setup(None);
    let config = Arc::get_mut(&mut e.config).unwrap();
    config.super_admins = vec![ADMIN];
    config.chats = (0..12).map(|n| CHAT - n).collect();
    admin::handle(&e, 0, &admin_message(ADMIN, "/admin"))
        .await
        .unwrap();
    let first = last_reply(&e);
    assert_eq!(
        buttons(&first)
            .iter()
            .filter(|button| button["callback_data"]
                .as_str()
                .unwrap()
                .starts_with("panel|"))
            .count(),
        10
    );
    click(&e, &next_page(&first).unwrap()).await;
    assert_eq!(
        buttons(&last_reply(&e))
            .iter()
            .filter(|button| button["callback_data"]
                .as_str()
                .unwrap()
                .starts_with("panel|"))
            .count(),
        2
    );
    click(&e, &callback(&last_reply(&e), "panel")).await;
    let group = callback(&last_reply(&e), "pending")
        .split('|')
        .nth(1)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    for action in ["pending", "audit"] {
        click(&e, &callback(&last_reply(&e), action)).await;
        assert_eq!(
            callback(&last_reply(&e), "panel"),
            format!("panel|{group}|")
        );
        click(&e, &callback(&last_reply(&e), "panel")).await;
    }
    assert_eq!(e.services.calls("sendMessage").len(), 1);
}

#[tokio::test]
async fn moderation_replaces_stale_buttons_and_repeated_clicks_do_not_repeat_punishment() {
    let (e, _) = setup(Some(0.75));
    review_cases(&e, 1).await;
    let case = cases(&e).remove(0);
    e.services.member(ADMIN, json!({"status":"creator"}));
    let ban = format!("ban|{CHAT}|{}", case.id);
    click(&e, &ban).await;
    let result = last_reply(&e);
    assert!(formatting::rendered(&result).contains("已"));
    let actions = buttons(&result);
    assert!(actions.iter().all(|button| {
        !matches!(
            button["callback_data"].as_str().unwrap().split('|').next(),
            Some("allow" | "ban")
        )
    }));
    assert_eq!(callback(&result, "pending"), format!("pending|{CHAT}|0"));
    assert_eq!(callback(&result, "panel"), format!("panel|{CHAT}|"));
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "banned");
    assert_eq!(e.services.calls("deleteMessage").len(), 1);
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    click(&e, &ban).await;
    click(&e, &format!("allow|{CHAT}|{}", case.id)).await;
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "banned");
    assert_eq!(e.services.calls("deleteMessage").len(), 1);
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert!(buttons(&last_reply(&e)).iter().all(|button| {
        !matches!(
            button["callback_data"].as_str().unwrap().split('|').next(),
            Some("allow" | "ban")
        )
    }));
    assert!(e.services.calls("sendMessage").is_empty());
}

#[tokio::test]
async fn forged_callback_context_and_revoked_permissions_never_mutate_or_edit() {
    let (mut e, _) = setup(Some(0.75));
    review_cases(&e, 1).await;
    let id = cases(&e)[0].id.clone();
    Arc::get_mut(&mut e.config).unwrap().chats.push(CHAT - 1);
    e.services.member(ADMIN, json!({"status":"creator"}));
    for action in ["ban", "spamoff"] {
        let valid = admin_callback(ADMIN, CHAT, action, &id);
        let mut invalid = vec![];
        let mut mismatch = valid.clone();
        mismatch["callback_query"]["message"]["chat"]["id"] = json!(ADMIN + 1);
        invalid.push(mismatch);
        let mut missing_id = valid.clone();
        missing_id["callback_query"]["message"]
            .as_object_mut()
            .unwrap()
            .remove("message_id");
        invalid.push(missing_id);
        for message_id in [0, -1] {
            let mut bad_id = valid.clone();
            bad_id["callback_query"]["message"]["message_id"] = json!(message_id);
            invalid.push(bad_id);
        }
        let mut group_message = valid.clone();
        group_message["callback_query"]["message"]["chat"] = json!({"id":CHAT,"type":"supergroup"});
        invalid.push(group_message);
        for update in invalid {
            e.services.calls.lock().unwrap().clear();
            assert_eq!(admin::target_chat(&update, &e.config), None);
            admin::handle(&e, CHAT, &update).await.unwrap();
            assert!(e.services.replies().is_empty());
            assert_eq!(cases(&e)[0].state, "review");
            assert!(e.settings(CHAT).unwrap().spam);
            assert!(e.services.calls("deleteMessage").is_empty());
            assert!(e.services.calls("banChatMember").is_empty());
        }
        // The dispatcher scope and callback payload must name the same group.
        e.services.calls.lock().unwrap().clear();
        admin::handle(&e, CHAT, &admin_callback(ADMIN, CHAT - 1, action, &id))
            .await
            .unwrap();
        assert!(e.services.replies().is_empty());
        assert_eq!(cases(&e)[0].state, "review");
        assert!(e.settings(CHAT).unwrap().spam);
    }
    e.services.member(ADMIN, json!({"status":"member"}));
    for action in ["panel", "ban", "spamoff"] {
        e.services.calls.lock().unwrap().clear();
        admin::handle(&e, CHAT, &admin_callback(ADMIN, CHAT, action, &id))
            .await
            .unwrap();
        assert!(e.services.replies().is_empty());
        assert_eq!(cases(&e)[0].state, "review");
        assert!(e.settings(CHAT).unwrap().spam);
    }
    drain(&e).await;
    assert!(e.services.calls("deleteMessage").is_empty());
    assert!(e.services.calls("banChatMember").is_empty());
}

#[tokio::test]
async fn case_ids_from_another_group_cannot_approve_or_punish_the_original_case() {
    let (mut e, _) = setup(Some(0.75));
    review_cases(&e, 1).await;
    let case = cases(&e).remove(0);
    let other = CHAT - 1;
    Arc::get_mut(&mut e.config).unwrap().chats.push(other);
    e.services.member(ADMIN, json!({"status":"creator"}));
    for action in ["case", "allow", "ban"] {
        admin::handle(&e, other, &admin_callback(ADMIN, other, action, &case.id))
            .await
            .unwrap();
        assert_eq!(cases(&e)[0].state, "review");
        assert!(
            e.store
                .get::<Case>(other, &format!("case:{}", case.id))
                .unwrap()
                .is_none()
        );
    }
    drain(&e).await;
    assert!(e.services.calls("deleteMessage").is_empty());
    assert!(e.services.calls("banChatMember").is_empty());
    assert!(e.services.calls("sendMessage").is_empty());
}

#[tokio::test]
async fn persisted_admin_callback_retries_editing_the_same_message_after_transient_failure() {
    let (e, time) = setup(None);
    e.services.member(ADMIN, json!({"status":"creator"}));
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("editMessageText".into(), 1);
    let mut update = admin_callback(ADMIN, CHAT, "panel", "");
    update["update_id"] = json!(9104);
    e.ingest(&update).unwrap();
    drain(&e).await;
    assert_eq!(e.services.calls("editMessageText").len(), 1);
    assert!(e.services.calls("sendMessage").is_empty());
    let stats = e.store.stats(CHAT).unwrap();
    assert_eq!(stats["pending"].as_i64().unwrap_or(0), 1);
    assert_eq!(stats["done"].as_i64().unwrap_or(0), 0);
    time.fetch_add(1000, Ordering::SeqCst);
    drain(&e).await;
    let edits = e.services.calls("editMessageText");
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0], edits[1]);
    for reply in &edits {
        assert_eq!(reply["chat_id"], ADMIN);
        assert_eq!(reply["message_id"], MENU_MESSAGE);
        formatting::rendered(reply);
    }
    assert!(e.services.calls("sendMessage").is_empty());
    let stats = e.store.stats(CHAT).unwrap();
    assert_eq!(stats["pending"].as_i64().unwrap_or(0), 0);
    assert_eq!(stats["done"].as_i64().unwrap_or(0), 1);
    assert_eq!(stats["dead"].as_i64().unwrap_or(0), 0);
    assert!(e.store.claim(e.now()).unwrap().is_none());
}
