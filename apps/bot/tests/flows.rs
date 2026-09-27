#[path = "flows/admin_audit.rs"]
mod admin_audit;
#[path = "flows/clock_skew.rs"]
mod clock_skew;
#[path = "flows/first_seen.rs"]
mod first_seen;
#[path = "flows/formatting.rs"]
mod formatting;
#[path = "flows/profiles_guest.rs"]
mod profiles_guest;
#[path = "flows/statistics.rs"]
mod statistics;
mod support;

use fuuki_iin_bot::{
    admin,
    api::{Classification, Services},
    config::Config,
    engine::Engine,
    error::{Error, Result},
    evidence,
    logging::{LogLevel, Logger},
    menus,
    model::{Case, Member, Session},
    statistics::Usage,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};
const CHAT: i64 = -1001234567890;
const USER: i64 = 42;
const NOW: i64 = 1_790_294_400;

struct Mock {
    calls: Mutex<Vec<(String, Value)>>,
    members: Mutex<HashMap<i64, Value>>,
    chats: Mutex<HashMap<i64, Value>>,
    probability: Mutex<Option<f64>>,
    usage: Mutex<Usage>,
    classify_transport_failures: Mutex<u32>,
    evidence: Mutex<Vec<Value>>,
    failures: Mutex<HashMap<String, u32>>,
    verdict: Mutex<Option<Value>>,
    after_verify: Mutex<Option<(Arc<AtomicI64>, i64)>>,
}
impl Mock {
    fn new(p: Option<f64>) -> Self {
        Self {
            calls: Mutex::new(vec![]),
            members: Mutex::new(HashMap::new()),
            chats: Mutex::new(HashMap::new()),
            probability: Mutex::new(p),
            usage: Mutex::new(Usage::default()),
            classify_transport_failures: Mutex::new(0),
            evidence: Mutex::new(vec![]),
            failures: Mutex::new(HashMap::new()),
            verdict: Mutex::new(None),
            after_verify: Mutex::new(None),
        }
    }
    fn calls(&self, method: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, v)| v.clone())
            .collect()
    }
    fn member(&self, user: i64, value: Value) {
        self.members.lock().unwrap().insert(user, value);
    }
}
impl Services for Mock {
    async fn siteverify(&self, token: &str, idempotency_key: &str) -> Result<Value> {
        self.telegram(
            "siteverify",
            json!({"token":token,"idempotency_key":idempotency_key}),
        )
        .await?;
        if let Some((time, at)) = self.after_verify.lock().unwrap().take() {
            time.store(at, Ordering::SeqCst);
        }
        Ok(self.verdict.lock().unwrap().clone().unwrap_or_else(|| json!({"success":token.starts_with("valid-"),"hostname":"verify.example.com","action":"join","cdata":token.strip_prefix("valid-").unwrap_or("")})))
    }
    async fn telegram(&self, method: &str, body: Value) -> Result<Value> {
        if method == "sendMessage" {
            formatting::rendered(&body);
        }
        self.calls
            .lock()
            .unwrap()
            .push((method.into(), body.clone()));
        if let Some(n) = self.failures.lock().unwrap().get_mut(method)
            && *n > 0
        {
            *n -= 1;
            return Err(Error::external("test_transient", false));
        }
        if method == "getChatAdministrators" {
            return Ok(Value::Array(
                self.members
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, member)| fuuki_iin_bot::api::is_admin(member))
                    .map(|(id, member)| {
                        let mut member = member.clone();
                        if member["user"].is_null() {
                            member["user"] = json!({"id":id,"is_bot":false});
                        }
                        member
                    })
                    .collect(),
            ));
        }
        if method == "getChat" {
            let chat = body["chat_id"].as_i64().unwrap();
            return Ok(self
                .chats
                .lock()
                .unwrap()
                .get(&chat)
                .cloned()
                .unwrap_or(json!({"id":chat,"type":"supergroup","title":"测试群"})));
        }
        if method == "getChatMember" {
            return Ok(self
                .members
                .lock()
                .unwrap()
                .get(&body["user_id"].as_i64().unwrap())
                .cloned()
                .unwrap_or(json!({"status":"member"})));
        }
        Ok(json!(true))
    }
    async fn classify(&self, evidence: &Value) -> Result<Classification> {
        self.evidence.lock().unwrap().push(evidence.clone());
        let mut failures = self.classify_transport_failures.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            return Err(Error::external("upstream_unavailable", false));
        }
        Ok(Classification {
            decision: self
                .probability
                .lock()
                .unwrap()
                .map(|p| (p, "test-model".into()))
                .ok_or(Error::external("test_jev_unavailable", false)),
            usage: self.usage.lock().unwrap().clone(),
        })
    }
}
fn setup(p: Option<f64>) -> (Engine<Mock>, Arc<AtomicI64>) {
    let time = Arc::new(AtomicI64::new(NOW));
    let clock = time.clone();
    (
        Engine {
            store: Arc::new(Store::memory().unwrap()),
            config: Arc::new(Config {
                telegram_token: "999:testing".into(),
                openrouter_key: "synthetic".into(),
                turnstile_secret: "synthetic-turnstile-secret".into(),
                turnstile_site_key: "synthetic-sitekey".into(),
                verification_url: "https://verify.example.com".into(),
                bot_username: "FuukiIinTestBot".into(),
                chats: vec![CHAT],
                super_admins: vec![],
                database: "data/test.sqlite".into(),
                jev_model: "test-model".into(),
                log_level: LogLevel::Info,
            }),
            services: Arc::new(Mock::new(p)),
            clock: Arc::new(move || clock.load(Ordering::SeqCst)),
            logger: Arc::new(Logger::with_writer(LogLevel::Info, &[], std::io::sink())),
        },
        time,
    )
}
async fn drain(e: &Engine<Mock>) {
    for _ in 0..100 {
        let Some(job) = e.store.claim(e.now()).unwrap() else {
            return;
        };
        e.run(&job).await.unwrap();
    }
    panic!("job loop did not drain");
}
fn message(id: i64) -> Value {
    json!({"message_id":id,"date":NOW,"chat":{"id":CHAT,"type":"supergroup"},"from":{"id":USER,"username":"new_user","first_name":"新成员"},"text":"Hello"})
}
fn cases(e: &Engine<Mock>) -> Vec<Case> {
    e.store.list(CHAT, "case:", None, 0, 100).unwrap()
}
fn join(date: i64) -> Value {
    json!({"date":date,"from":{"id":USER},"chat":{"id":CHAT},"user_chat_id":USER})
}
fn session(e: &Engine<Mock>) -> Session {
    let n = e
        .store
        .get::<Value>(CHAT, &format!("latest:{USER}"))
        .unwrap()
        .unwrap();
    e.current_session(CHAT, n["nonce"].as_str().unwrap())
        .unwrap()
        .unwrap()
}
fn submission(e: &Engine<Mock>, s: &Session, sender: i64) -> Value {
    json!({"message_id":20,"date":e.now(),"chat":{"id":sender,"type":"private"},"from":{"id":sender},"web_app_data":{"data":json!({"v":2,"session":s.claims.nonce,"chat":s.claims.chat,"token":format!("valid-{}",s.claims.nonce)}).to_string()}})
}

#[tokio::test]
async fn debug_logs_inputs_and_retry_outcomes_without_changing_command_execution() {
    let (mut e, time) = setup(Some(0.01));
    let output = support::Capture::default();
    e.logger = Arc::new(Logger::with_writer(LogLevel::Debug, &[], output.clone()));
    let mut msg = message(950);
    msg["chat"] = json!({"id":USER,"type":"private"});
    msg["text"] = json!("/ping");
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("sendMessage".into(), 1);
    e.ingest(&json!({"update_id":950,"message":msg})).unwrap();
    drain(&e).await;
    time.fetch_add(60, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(e.services.calls("sendMessage").len(), 2);
    let records = output.records();
    let input = records
        .iter()
        .find(|r| r["event"] == "telegram.update")
        .unwrap();
    assert_eq!(input["body"]["message"]["text"], "/ping");
    assert_eq!(input["fields"]["user_id"], USER);
    let error = records.iter().find(|r| r["event"] == "job.error").unwrap();
    assert_eq!(error["fields"]["exhausted"], false);
    assert!(error["fields"]["retry_after_seconds"].as_u64().unwrap() > 0);
    assert!(records.iter().any(
        |r| r["event"] == "job.complete" && r["fields"]["job_id"] == error["fields"]["job_id"]
    ));
    assert_eq!(output.flushes(), records.len());
}

#[tokio::test]
async fn first_message_only_and_actual_sender_banned_for_inline_bot() {
    let (e, _) = setup(Some(0.8));
    e.joined(CHAT, USER, NOW, true).unwrap();
    let mut m = message(10);
    m["via_bot"] = json!({"id":88,"is_bot":true,"username":"inline_bot"});
    m["reply_markup"] =
        json!({"inline_keyboard":[[{"text":"pay","url":"https://example.invalid"}]]});
    e.message(CHAT, &m).unwrap();
    e.message(CHAT, &m).unwrap();
    e.message(CHAT, &message(11)).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(
        e.services.evidence.lock().unwrap()[0]["message"]["reply_markup"]["inline_keyboard"][0][0]
            ["text"],
        "pay"
    );
    assert_eq!(e.services.calls("banChatMember")[0]["user_id"], USER);
    assert_eq!(e.services.calls("deleteMessage")[0]["message_id"], 10);
    assert_eq!(cases(&e)[0].state, "banned");
}

#[tokio::test]
async fn emoji_body_with_spam_in_multiple_buttons_reaches_the_model_intact() {
    let (e, _) = setup(Some(0.95));
    e.joined(CHAT, USER, NOW, true).unwrap();
    let mut m = message(12);
    m["text"] = json!("💰".repeat(300));
    m["via_bot"] =
        json!({"id":88,"is_bot":true,"username":"synthetic_inline_bot","first_name":"测试 Bot"});
    let mut rows: Vec<Value> = [
        "💰 财富入口 💰",
        "💰 0 本金搬砖 💰",
        "💰 抓紧致富 💰",
        "💰 一天七八万随便拿 💰",
        "💰 大胆干来一天一万 💰",
        "💰 过年前带你提迈巴赫 💰",
        "💰 无风险项目 💰",
        "💰 一天搞个几千 💰",
        "💰 2026 年想翻身找我就对了 💰",
        "💰 当代年轻人的致富之路 💰",
        "💰 翻身跃迁，就在当下 💰",
        "💰 别再靠死工资！副业增收 💰",
        "💰 一天 6000，轻松到手 💰",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, text)| json!([{"text":text,"url":format!("https://example.invalid/promo/{i}")}]))
    .collect();
    rows.push(json!([{"text":"立即领取 / Claim now","callback_data":"claim:synthetic"}]));
    m["reply_markup"] = json!({"inline_keyboard":rows});
    e.message(CHAT, &m).unwrap();
    drain(&e).await;
    let evidence = e.services.evidence.lock().unwrap()[0].clone();
    assert_eq!(evidence["message"]["text"], m["text"]);
    assert_eq!(evidence["message"]["reply_markup"], m["reply_markup"]);
    assert_eq!(evidence["via_bot"], m["via_bot"]);
    assert_eq!(evidence["user"]["id"], USER);
    assert_eq!(evidence["user"]["username"], m["from"]["username"]);
    assert_eq!(evidence["user"]["first_name"], m["from"]["first_name"]);
    let request = evidence::classify_request(&evidence, "test-model").unwrap();
    assert_eq!(request["state"]["message_for_screening"], evidence);
    // Synthetic probability verifies routing, not Jev's real-world accuracy.
    assert_eq!(e.services.calls("banChatMember")[0]["user_id"], USER);
    assert_eq!(e.services.calls("deleteMessage")[0]["message_id"], 12);
}
#[test]
fn probability_boundaries_and_invalid_values() {
    let settings = fuuki_iin_bot::model::Settings::default();
    assert_eq!((settings.review, settings.ban), (0.6, 0.8));
    for (p, state) in [
        (0.0, "allowed"),
        (0.599999, "allowed"),
        (0.6, "review"),
        (0.799999, "review"),
        (0.8, "enforcing"),
        (1.0, "enforcing"),
    ] {
        assert_eq!(
            evidence::decision(p, settings.review, settings.ban).unwrap(),
            state
        );
    }
    for p in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(evidence::decision(p, settings.review, settings.ban).is_err());
    }
}

#[tokio::test]
async fn configured_thresholds_apply_to_new_cases_without_changing_pending_cases() {
    let (e, _) = setup(Some(0.85));
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(
        &e,
        CHAT,
        &admin_message(7, &format!("/thresholds {CHAT} 60 90")),
    )
    .await
    .unwrap();
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(20)).unwrap();
    admin::handle(
        &e,
        CHAT,
        &admin_message(7, &format!("/thresholds {CHAT} 60 80")),
    )
    .await
    .unwrap();
    e.joined(CHAT, USER + 1, NOW, true).unwrap();
    let mut next = message(21);
    next["from"]["id"] = json!(USER + 1);
    e.message(CHAT, &next).unwrap();
    drain(&e).await;
    let all = cases(&e);
    let pending = all.iter().find(|c| c.message == 20).unwrap();
    assert_eq!(
        (pending.state.as_str(), pending.ban_threshold),
        ("review", 0.9)
    );
    let new = all.iter().find(|c| c.message == 21).unwrap();
    assert_eq!((new.state.as_str(), new.ban_threshold), ("banned", 0.8));
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert_eq!(e.services.calls("banChatMember")[0]["user_id"], USER + 1);
    assert_eq!(e.services.calls("deleteMessage").len(), 1);
    assert_eq!(e.services.calls("deleteMessage")[0]["message_id"], 21);
}
#[tokio::test]
async fn unobserved_members_services_anonymous_and_edited_updates_are_skipped() {
    let (e, _) = setup(Some(1.0));
    e.message(CHAT, &message(1)).unwrap();
    e.joined(CHAT, USER, NOW, true).unwrap();
    let mut m = message(2);
    m["sender_chat"] = json!({"id":CHAT});
    e.message(CHAT, &m).unwrap();
    e.ingest(&json!({"update_id":3,"edited_message":message(3)}))
        .unwrap();
    e.message(
        CHAT,
        &json!({"message_id":4,"date":NOW,"from":{"id":USER},"pinned_message":message(1)}),
    )
    .unwrap();
    drain(&e).await;
    assert!(cases(&e).is_empty());
    e.message(CHAT, &message(5)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e).len(), 1);
}
#[tokio::test]
async fn review_requires_current_admin_permissions_and_first_decision_wins() {
    let (e, _) = setup(Some(0.75));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    let callback = json!({"callback_query":{"id":"cb","from":{"id":7},"message":{"chat":{"id":7,"type":"private"}},"data":format!("ban|{CHAT}|{id}")}});
    admin::handle(&e, CHAT, &callback).await.unwrap();
    assert_eq!(cases(&e)[0].state, "review");
    e.services.member(
        7,
        json!({"status":"administrator","can_delete_messages":true,"can_restrict_members":false}),
    );
    admin::handle(&e, CHAT, &callback).await.unwrap();
    assert_eq!(cases(&e)[0].state, "review");
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, CHAT, &callback).await.unwrap();
    assert!(!admin::decide(&e, CHAT, &id, false, 8).unwrap());
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "banned");
    assert_eq!(cases(&e)[0].actor, Some(7));
}

fn admin_message(user: i64, text: &str) -> Value {
    json!({"message":{"message_id":1000,"chat":{"id":user,"type":"private"},"from":{"id":user},"text":text}})
}
fn admin_callback(user: i64, chat: i64, action: &str, id: &str) -> Value {
    json!({"callback_query":{"id":"super-admin-test","from":{"id":user},"message":{"chat":{"id":user,"type":"private"}},"data":format!("{action}|{chat}|{id}")}})
}

fn has_admin_menu(call: &Value) -> bool {
    call["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["command"] == "admin")
}

#[tokio::test]
async fn public_scopes_never_advertise_management_and_clear_language_overrides() {
    let (e, _) = setup(None);
    menus::install_public(e.services.as_ref(), &e.config)
        .await
        .unwrap();
    let calls = e.services.calls("setMyCommands");
    assert_eq!(calls.len(), 5);
    for call in &calls {
        assert!(!has_admin_menu(call));
        assert!(
            call["commands"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| { matches!(c["command"].as_str(), Some("ping" | "whoami" | "help")) })
        );
        for language in ["en", "zh"] {
            assert!(e.services.calls("deleteMyCommands").iter().any(|delete| {
                delete["scope"] == call["scope"] && delete["language_code"] == language
            }));
        }
    }
    assert!(calls.iter().any(|c| c["scope"]["type"] == "default"));
    assert!(
        calls
            .iter()
            .any(|c| c["scope"]["type"] == "all_private_chats")
    );
}

#[tokio::test]
async fn ordinary_users_cannot_discover_admin_commands_by_typing_or_callbacks() {
    let (e, _) = setup(None);
    for command in [
        "admin",
        "pending",
        "case",
        "health",
        "audit",
        "thresholds",
        "retry",
        "unban",
    ] {
        for chat in [0, CHAT] {
            let text = if chat == 0 {
                format!("/{command}")
            } else {
                format!("/{command} {chat}")
            };
            admin::handle(&e, chat, &admin_message(USER, &text))
                .await
                .unwrap();
        }
    }
    admin::handle(
        &e,
        CHAT,
        &admin_message(USER, &format!("/start admin_{CHAT}")),
    )
    .await
    .unwrap();
    admin::handle(&e, CHAT, &admin_callback(USER, CHAT, "panel", ""))
        .await
        .unwrap();
    assert!(e.services.calls("sendMessage").is_empty());
    assert!(e.services.calls("setMyCommands").is_empty());
    assert_eq!(e.services.calls("answerCallbackQuery")[0]["text"], "不可用");
}

#[tokio::test]
async fn admin_menus_are_private_and_revoked_without_using_menu_state_for_authorization() {
    let (e, _) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, 0, &admin_message(7, "/start"))
        .await
        .unwrap();
    let grants = e.services.calls("setMyCommands");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["scope"], json!({"type":"chat","chat_id":7}));
    assert!(has_admin_menu(&grants[0]));
    assert!(e.services.calls("sendMessage").is_empty());
    menus::refresh(&e).await.unwrap();
    assert_eq!(e.services.calls("setMyCommands").len(), 1);

    e.services.member(7, json!({"status":"member"}));
    // Before a periodic refresh, the live ACL still rejects old buttons.
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "spamoff", ""))
        .await
        .unwrap();
    assert!(e.settings(CHAT).unwrap().spam);
    assert!(e.services.calls("sendMessage").is_empty());
    assert!(!has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));

    e.services.member(7, json!({"status":"administrator"}));
    menus::refresh(&e).await.unwrap();
    assert!(has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));
    e.services.member(7, json!({"status":"left"}));
    menus::refresh(&e).await.unwrap();
    assert!(!has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));
}

#[tokio::test]
async fn persisted_menu_intent_survives_partial_api_failure_and_config_removal() {
    let (mut e, _) = setup(None);
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![7];
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("deleteMyCommands".into(), 1);
    assert!(
        admin::handle(&e, 0, &admin_message(7, "/start"))
            .await
            .is_err()
    );
    assert!(has_admin_menu(&e.services.calls("setMyCommands")[0]));
    assert!(e.store.get::<Value>(0, "menu:7").unwrap().unwrap()["applied"].is_null());
    Arc::get_mut(&mut e.config).unwrap().super_admins.clear();
    menus::refresh(&e).await.unwrap();
    assert!(!has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));

    e.services.member(7, json!({"status":"creator"}));
    menus::refresh(&e).await.unwrap();
    assert!(has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));
    Arc::get_mut(&mut e.config).unwrap().chats.clear();
    menus::refresh(&e).await.unwrap();
    assert!(!has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));
}

#[tokio::test]
async fn menu_refresh_fails_closed_but_keeps_configured_super_admins() {
    let (mut e, _) = setup(None);
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![8];
    e.services.member(7, json!({"status":"administrator"}));
    for user in [7, 8] {
        admin::handle(&e, 0, &admin_message(user, "/start"))
            .await
            .unwrap();
    }
    e.services.calls.lock().unwrap().clear();
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChatAdministrators".into(), 1);
    menus::refresh(&e).await.unwrap();
    let revoked = e.services.calls("setMyCommands");
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0]["scope"]["chat_id"], 7);
    assert!(!has_admin_menu(&revoked[0]));
    menus::refresh(&e).await.unwrap();
    assert!(has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));
}

#[tokio::test]
async fn demotion_event_refreshes_previously_granted_menu() {
    let (e, _) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, 0, &admin_message(7, "/start"))
        .await
        .unwrap();
    e.services.member(7, json!({"status":"member"}));
    e.ingest(&json!({"update_id":1800,"chat_member":{"chat":{"id":CHAT},"date":NOW,"old_chat_member":{"status":"administrator","user":{"id":7}},"new_chat_member":{"status":"member","user":{"id":7}}}})).unwrap();
    drain(&e).await;
    assert!(!has_admin_menu(
        e.services.calls("setMyCommands").last().unwrap()
    ));
}

#[tokio::test]
async fn review_and_failure_notices_only_reach_current_human_admins_and_super_admins() {
    let (mut e, _) = setup(Some(0.7));
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![7, 8];
    e.services.member(7, json!({"status":"creator"}));
    e.services.member(
        9,
        json!({"status":"administrator","user":{"id":9,"is_bot":true}}),
    );
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1801)).unwrap();
    e.queue(CHAT, "alert", "private-only", json!({})).unwrap();
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 4); // Two recipients, no duplicate for admin + super admin.
    assert!(
        replies
            .iter()
            .all(|r| r["chat_id"] == 7 || r["chat_id"] == 8)
    );
    assert_eq!(
        replies
            .iter()
            .filter(|r| !r["reply_markup"].is_null())
            .count(),
        2
    );
    assert_chinese_admin_replies(&e);
}

#[tokio::test]
async fn queued_notices_recheck_authority_and_never_fall_back_to_a_group() {
    let (mut e, time) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    e.queue(CHAT, "alert", "demotion", json!({})).unwrap();
    let job = e.store.claim(NOW).unwrap().unwrap();
    e.run(&job).await.unwrap();
    e.services.member(7, json!({"status":"member"}));
    drain(&e).await;
    assert!(e.services.calls("sendMessage").is_empty());

    e.services.member(7, json!({"status":"creator"}));
    e.queue(CHAT, "alert", "blocked-dm", json!({})).unwrap();
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("sendMessage".into(), 20);
    for _ in 0..8 {
        drain(&e).await;
        time.fetch_add(600, Ordering::SeqCst);
    }
    let calls = e.services.calls("sendMessage");
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|r| r["chat_id"] == 7));
    assert_eq!(e.store.stats(CHAT).unwrap()["dead"], 1);
    assert!(e.store.claim(e.now()).unwrap().is_none());

    e.services.failures.lock().unwrap().clear();
    e.services.calls.lock().unwrap().clear();
    e.queue(CHAT, "alert", "removed-group", json!({})).unwrap();
    let job = e.store.claim(e.now()).unwrap().unwrap();
    e.run(&job).await.unwrap();
    Arc::get_mut(&mut e.config).unwrap().chats.clear();
    drain(&e).await;
    assert!(e.services.calls("sendMessage").is_empty());
}

fn assert_bilingual_replies(e: &Engine<Mock>) {
    assert_reply_language(e, true, None);
}
fn assert_chinese_admin_replies(e: &Engine<Mock>) {
    assert_reply_language(e, false, None);
}
fn assert_reply_language(e: &Engine<Mock>, bilingual: bool, recipient: Option<i64>) {
    fn text(value: &Value, bilingual: bool) {
        let value = value.as_str().unwrap();
        assert!(
            value
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "missing Chinese: {value}"
        );
        if bilingual {
            assert!(
                value.chars().any(|c| c.is_ascii_alphabetic()),
                "missing English: {value}"
            );
        } else {
            for old in [
                " / Admin",
                " / Group",
                " / Spam",
                " / Pending",
                "Read-only",
                "Unavailable",
                "No messages",
                "Requires ",
            ] {
                assert!(!value.contains(old), "unexpected English UI: {value}");
            }
        }
    }
    for (method, body) in e.services.calls.lock().unwrap().iter() {
        if recipient.is_some_and(|id| body["chat_id"] != id) {
            continue;
        }
        if method == "sendMessage" || method == "answerCallbackQuery" {
            if method == "sendMessage" && formatting::rendered(body) == "Pong! 🏓" {
                continue;
            }
            text(&body["text"], bilingual);
            if method == "answerCallbackQuery" {
                assert!(body["text"].as_str().unwrap().chars().count() <= 200);
            }
            for key in ["inline_keyboard", "keyboard"] {
                if let Some(rows) = body["reply_markup"][key].as_array() {
                    for row in rows {
                        for button in row.as_array().unwrap() {
                            text(&button["text"], bilingual);
                        }
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn super_admin_can_read_all_configured_groups_without_group_admin_status() {
    let (mut e, _) = setup(Some(0.75));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    let other = CHAT - 1;
    let config = Arc::get_mut(&mut e.config).unwrap();
    config.super_admins = vec![7];
    config.chats.push(other);
    e.services.member(7, json!({"status":"left"}));
    e.services.calls.lock().unwrap().clear();

    admin::handle(&e, 0, &admin_message(7, "/admin"))
        .await
        .unwrap();
    let list = e.services.calls("sendMessage");
    let rows = list[0]["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0]["callback_data"], format!("panel|{CHAT}|"));
    assert_eq!(rows[1][0]["callback_data"], format!("panel|{other}|"));
    assert!(e.services.calls("getChatMember").is_empty());

    for group in [CHAT, other] {
        for command in ["admin", "pending", "health", "audit"] {
            admin::handle(&e, group, &admin_message(7, &format!("/{command} {group}")))
                .await
                .unwrap();
        }
        for action in ["panel", "pending", "health", "audit"] {
            admin::handle(&e, group, &admin_callback(7, group, action, ""))
                .await
                .unwrap();
        }
    }
    admin::handle(&e, CHAT, &admin_message(7, &format!("/case {CHAT} {id}")))
        .await
        .unwrap();
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 18);
    assert!(replies.iter().all(|reply| reply["chat_id"] == 7));
    let texts = replies
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(texts.contains("Hello"));
    assert!(texts.contains("分类完成"));
    assert!(texts.contains("Bot 状态"));
    assert!(texts.contains("设置仅供查看"));
    assert!(!texts.contains("/thresholds"));
    for reply in &replies {
        let markup = reply["reply_markup"].to_string();
        for action in ["spamon|", "spamoff|", "capon|", "capoff|", "ban|", "allow|"] {
            assert!(!markup.contains(action));
        }
    }
    assert_eq!(cases(&e)[0].state, "review");
    assert_chinese_admin_replies(&e);
}

#[tokio::test]
async fn super_admin_read_access_does_not_bypass_mutation_permissions() {
    let (mut e, _) = setup(Some(0.75));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![7];
    e.services.member(7, json!({"status":"member"}));
    e.services.calls.lock().unwrap().clear();
    for action in ["allow", "ban", "spamon", "spamoff", "capon", "capoff"] {
        admin::handle(&e, CHAT, &admin_callback(7, CHAT, action, &id))
            .await
            .unwrap();
    }
    for command in [
        format!("/thresholds {CHAT} 10 20"),
        format!("/retry {CHAT}"),
        format!("/unban {CHAT} {USER}"),
    ] {
        admin::handle(&e, CHAT, &admin_message(7, &command))
            .await
            .unwrap();
    }
    // API failure preserves local read access, but cannot enable mutations.
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("getChatMember".into(), 10);
    admin::handle(&e, CHAT, &admin_message(7, &format!("/case {CHAT} {id}")))
        .await
        .unwrap();
    assert!(
        e.services.calls("sendMessage").last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("Hello")
    );
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "ban", &id))
        .await
        .unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "review");
    assert!(e.settings(CHAT).unwrap().spam);
    assert!(e.settings(CHAT).unwrap().captcha);
    assert_eq!(e.settings(CHAT).unwrap().review, 0.6);
    for method in ["deleteMessage", "banChatMember", "unbanChatMember"] {
        assert!(e.services.calls(method).is_empty());
    }
    // A super admin who also has the required group permissions can act.
    e.services.failures.lock().unwrap().clear();
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "allow", &id))
        .await
        .unwrap();
    assert_eq!(cases(&e)[0].state, "allowed");
    assert_chinese_admin_replies(&e);
}

#[tokio::test]
async fn super_admin_requires_actual_sender_id_private_context_and_current_config() {
    let (mut e, _) = setup(None);
    Arc::get_mut(&mut e.config).unwrap().super_admins = vec![7];
    let command = format!("/admin {CHAT} 7");
    let mut forged = admin_message(8, &command);
    forged["message"]["from"]["username"] = json!("7");
    forged["message"]["forward_origin"] = json!({"type":"user","sender_user":{"id":7}});
    admin::handle(&e, CHAT, &forged).await.unwrap();
    admin::handle(&e, CHAT, &admin_callback(8, CHAT, "panel", "7"))
        .await
        .unwrap();
    assert!(e.services.calls("sendMessage").is_empty());
    assert_eq!(e.services.calls("answerCallbackQuery")[0]["text"], "不可用");
    assert_eq!(
        e.services.calls("answerCallbackQuery")[0]["show_alert"],
        true
    );
    e.services.calls.lock().unwrap().clear();
    let mut group = admin_message(7, &command);
    group["message"]["chat"] = json!({"id":CHAT,"type":"supergroup"});
    admin::handle(&e, CHAT, &group).await.unwrap();
    let mut bot = admin_message(7, &command);
    bot["message"]["from"]["is_bot"] = json!(true);
    admin::handle(&e, CHAT, &bot).await.unwrap();
    admin::handle(
        &e,
        CHAT - 1,
        &admin_message(7, &format!("/admin {}", CHAT - 1)),
    )
    .await
    .unwrap();
    assert!(e.services.calls.lock().unwrap().is_empty());
    Arc::get_mut(&mut e.config).unwrap().super_admins.clear();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "panel", ""))
        .await
        .unwrap();
    assert_eq!(
        e.services.calls("answerCallbackQuery")[0]["show_alert"],
        true
    );
    assert!(e.services.calls("sendMessage").is_empty());
}

#[tokio::test]
async fn administrator_promotion_before_enforcement_prevents_ban() {
    let (e, _) = setup(Some(1.0));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    let job = e.store.claim(NOW).unwrap().unwrap();
    e.run(&job).await.unwrap();
    e.services.member(USER, json!({"status":"administrator"}));
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "protected");
    assert!(e.services.calls("banChatMember").is_empty());
}
#[tokio::test]
async fn partial_enforcement_retries_only_unfinished_action() {
    let (e, time) = setup(Some(1.0));
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("deleteMessage".into(), 1);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    assert!(cases(&e)[0].banned);
    assert!(!cases(&e)[0].deleted);
    time.store(NOW + 10, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "banned");
    assert_eq!(e.services.calls("banChatMember").len(), 1);
    assert_eq!(e.services.calls("deleteMessage").len(), 2);
}
#[tokio::test]
async fn jev_failure_becomes_manual_review_without_punishment() {
    let (e, time) = setup(None);
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    for n in [0, 10, 30] {
        time.store(NOW + n, Ordering::SeqCst);
        drain(&e).await;
    }
    assert_eq!(cases(&e)[0].state, "review");
    assert!(cases(&e)[0].probability.is_none());
    assert!(e.services.calls("banChatMember").is_empty());
}
#[tokio::test]
async fn old_case_cannot_ban_user_after_rejoin() {
    let (e, _) = setup(Some(0.7));
    e.joined(CHAT, USER, NOW - 10, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let id = cases(&e)[0].id.clone();
    e.joined(CHAT, USER, NOW, true).unwrap();
    admin::decide(&e, CHAT, &id, true, 7).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "expired");
    assert!(e.services.calls("banChatMember").is_empty());
}
#[tokio::test]
async fn turnstile_submission_approves_once_and_keyboard_is_correct_launch_mode() {
    let (e, _) = setup(Some(0.1));
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    drain(&e).await;
    let sent = e.services.calls("sendMessage");
    assert!(
        sent[0]["reply_markup"]["keyboard"][0][0]["web_app"]["url"]
            .as_str()
            .unwrap()
            .contains("/verify?session=")
    );
    assert!(sent[0]["reply_markup"]["inline_keyboard"].is_null());
    let s = session(&e);
    let m = submission(&e, &s, USER);
    e.receive_verification(&m).unwrap();
    e.receive_verification(&m).unwrap();
    drain(&e).await;
    e.receive_verification(&m).unwrap();
    drain(&e).await;
    assert_eq!(e.services.calls("approveChatJoinRequest").len(), 1);
    assert_eq!(session(&e).state, "approved");
    assert_eq!(e.services.calls("siteverify").len(), 1);
    assert!(session(&e).verification.is_none());
    assert_eq!(sent[0]["chat_id"], USER);
    assert!(
        sent[0]["reply_markup"]["keyboard"][0][0]["web_app"]["url"]
            .as_str()
            .unwrap()
            .contains("sitekey=synthetic-sitekey")
    );
    e.message(CHAT, &message(5)).unwrap();
    e.joined(CHAT, USER, NOW + 1, true).unwrap();
    e.message(CHAT, &message(6)).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn forged_other_user_expired_and_replaced_submissions_cannot_approve() {
    let (e, time) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    let old = session(&e);
    e.receive_verification(&submission(&e, &old, 777)).unwrap();
    let mut forged = submission(&e, &old, USER);
    forged["web_app_data"]["data"] = json!("{\"receipt\":\"fake\"}");
    e.receive_verification(&forged).unwrap();
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    time.store(NOW + 1, Ordering::SeqCst);
    e.join_request(CHAT, &join(NOW + 1), 101).unwrap();
    e.receive_verification(&submission(&e, &old, USER)).unwrap();
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    let latest = session(&e);
    let m = submission(&e, &latest, USER);
    time.store(NOW + 601, Ordering::SeqCst);
    e.receive_verification(&m).unwrap();
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
}
#[tokio::test]
async fn old_protocol_cannot_approve_and_switch_off_blocks_verification() {
    let (e, _) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    let s = session(&e);
    let mut m = submission(&e, &s, USER);
    m["web_app_data"]["data"] = json!({"receipt":"obsolete-v1-ticket"}).to_string().into();
    e.receive_verification(&m).unwrap();
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    e.receive_verification(&submission(&e, &s, USER)).unwrap();
    let mut settings = e.settings(CHAT).unwrap();
    settings.captcha = false;
    e.store.put(CHAT, "settings", &settings).unwrap();
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
}
#[test]
fn durable_inbox_offset_and_recovery_are_idempotent() {
    let store = Store::memory().unwrap();
    let update = json!({"update_id":7,"message":message(1)});
    store
        .persist_updates(std::slice::from_ref(&update), NOW)
        .unwrap();
    assert_eq!(store.offset().unwrap(), 8);
    let first = store.claim(NOW).unwrap().unwrap();
    store.persist_updates(&[update], NOW).unwrap();
    assert!(store.claim(NOW).unwrap().is_none());
    store.recover(NOW + 1).unwrap();
    let recovered = store.claim(NOW + 1).unwrap().unwrap();
    assert_eq!(first.id, recovered.id);
    assert_eq!(recovered.attempts, 2);
    store.complete(&recovered.id).unwrap();
    assert!(store.claim(NOW + 2).unwrap().is_none());
}
#[tokio::test]
async fn evidence_retention_preserves_first_message_marker() {
    let (e, _) = setup(Some(0.1));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    e.store.prune(NOW + 604801).unwrap();
    assert!(cases(&e)[0].evidence.is_none());
    e.store.prune(NOW + 3_196_801).unwrap();
    assert!(cases(&e).is_empty());
    assert_eq!(
        e.store
            .get::<Member>(CHAT, &format!("member:{USER}"))
            .unwrap()
            .unwrap()
            .first_message,
        Some(1)
    );
}
#[test]
fn configuration_literals_never_execute_shell_and_commands_are_scoped() {
    let env = fuuki_iin_bot::config::parse_env("# ignored\nA='$(bad)'\nB=hello\n").unwrap();
    assert_eq!(env["A"], "$(bad)");
    assert!(admin::command("/admin@AnotherBot", "FuukiIinTestBot").is_none());
    assert_eq!(
        admin::command("/admin@FuukiIinTestBot -123", "FuukiIinTestBot")
            .unwrap()
            .1,
        vec!["-123"]
    );
}

#[tokio::test]
async fn public_commands_work_in_private_without_admin_access_or_model_calls() {
    let (e, _) = setup(None);
    for (id, text) in [(70, "/ping"), (71, "/whoami@fuukiiintestbot -123")] {
        let mut m = message(id);
        m["chat"] = json!({"id":USER,"type":"private"});
        m["text"] = json!(text);
        m["from"]["first_name"] = json!("<林>\n开发者");
        m["from"]["last_name"] = json!("小明");
        m["reply_to_message"] = json!({"from":{"id":123,"first_name":"另一个人"}});
        m["forward_origin"] = json!({"type":"user","sender_user":{"id":456}});
        m["via_bot"] = json!({"id":888,"is_bot":true});
        e.ingest(&json!({"update_id":id,"message":m})).unwrap();
    }
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 2);
    assert_eq!(formatting::rendered(&replies[0]), "Pong! 🏓");
    assert_eq!(replies[0]["chat_id"], USER);
    assert_eq!(replies[0]["reply_parameters"]["message_id"], 70);
    assert_eq!(
        replies[1]["text"],
        "*用户 ID / User ID*：`42`\n*昵称 / Name*：<林\\> 开发者 小明\n*用户名 / Username*：`@new_user`\n*聊天 ID / Chat ID*：`42`\n*聊天类型 / Chat type*：私聊 / Private"
    );
    assert_eq!(replies[1]["parse_mode"], "MarkdownV2");
    assert_eq!(replies[1]["link_preview_options"]["is_disabled"], true);
    assert!(e.services.calls("getChatMember").is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
    assert_bilingual_replies(&e);
}

#[tokio::test]
async fn ordinary_private_messages_do_not_reveal_a_service_introduction() {
    let (e, _) = setup(None);
    for (id, value) in ["/start", "hello", "/unknown", "/unknown -1001234567890", ""]
        .iter()
        .enumerate()
    {
        let mut update = admin_message(USER, value);
        update["update_id"] = json!(1100 + id);
        e.ingest(&update).unwrap();
    }
    drain(&e).await;
    assert!(e.services.calls("sendMessage").is_empty());
    assert!(e.services.calls("setMyCommands").is_empty());
    let mut help = admin_message(USER, "/help");
    help["update_id"] = json!(1110);
    e.ingest(&help).unwrap();
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 1);
    let help = replies[0]["text"].as_str().unwrap();
    assert!(help.contains("/ping") && help.contains("/whoami"));
    for internal in [
        "/admin",
        "管理员",
        "Admin",
        "OpenRouter",
        "Jev",
        "TypeSafe",
        "Turnstile",
        "首条消息",
        "反垃圾",
    ] {
        assert!(!help.contains(internal));
    }
    assert_bilingual_replies(&e);
}

#[tokio::test]
async fn verification_stays_bilingual_while_review_notices_are_chinese() {
    let (e, _) = setup(None);
    e.services.member(7, json!({"status":"creator"}));
    e.join_request(CHAT, &join(NOW), 1120).unwrap();
    drain(&e).await;
    let s = session(&e);
    for rate_limited in [false, true] {
        e.queue(
            CHAT,
            "verification_rejected",
            &format!("bilingual-{rate_limited}"),
            json!({"user":USER,"id":s.claims.nonce,"rate_limited":rate_limited}),
        )
        .unwrap();
    }
    e.queue(0, "verification_rejected", "invalid", json!({"user":USER}))
        .unwrap();
    drain(&e).await;
    e.receive_verification(&submission(&e, &s, USER)).unwrap();
    drain(&e).await;
    assert_eq!(session(&e).state, "approved");
    e.message(CHAT, &message(1121)).unwrap();
    // Exercise a review notification with a score and one without a score.
    let mut case = cases(&e)[0].clone();
    case.state = "review".into();
    for (index, probability) in [None, Some(0.7)].into_iter().enumerate() {
        case.probability = probability;
        e.store
            .put(CHAT, &format!("case:{}", case.id), &case)
            .unwrap();
        e.queue(
            CHAT,
            "notify",
            &format!("bilingual-{index}"),
            json!({"id":case.id}),
        )
        .unwrap();
        drain(&e).await;
    }
    e.queue(CHAT, "alert", "bilingual", json!({})).unwrap();
    drain(&e).await;
    assert!(
        e.services
            .calls("sendMessage")
            .iter()
            .all(|r| r["chat_id"] == USER || r["chat_id"] == 7)
    );
    assert_reply_language(&e, true, Some(USER));
    assert_reply_language(&e, false, Some(7));
}

#[tokio::test]
async fn group_command_replies_in_topic_and_keeps_first_message_moderation() {
    let (e, _) = setup(Some(0.1));
    e.joined(CHAT, USER, NOW, true).unwrap();
    let mut m = message(72);
    m["text"] = json!("/whoami@FuukiIinTestBot");
    m["message_thread_id"] = json!(20);
    m["is_topic_message"] = json!(true);
    let update = json!({"update_id":72,"message":m});
    e.ingest(&update).unwrap();
    e.ingest(&update).unwrap();
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["chat_id"], CHAT);
    assert_eq!(replies[0]["message_thread_id"], 20);
    assert_eq!(replies[0]["reply_parameters"]["message_id"], 72);
    assert_eq!(
        replies[0]["reply_parameters"]["allow_sending_without_reply"],
        true
    );
    assert!(formatting::rendered(&replies[0]).contains(&format!("聊天 ID / Chat ID：{CHAT}")));
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(cases(&e)[0].message, 72);
    e.message(CHAT, &message(73)).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
    assert_eq!(cases(&e)[0].state, "allowed");
}

#[tokio::test]
async fn whoami_handles_absent_username_and_anonymous_sender_chat() {
    let (e, _) = setup(None);
    let mut m = message(74);
    m["text"] = json!("/whoami");
    m["from"] = json!({"id":USER,"first_name":"小林"});
    e.ingest(&json!({"update_id":74,"message":m})).unwrap();
    m["message_id"] = json!(75);
    m["sender_chat"] = json!({"id":CHAT,"type":"supergroup","title":"测试群"});
    m["from"] = json!({"id":1087968824_i64,"is_bot":true,"username":"GroupAnonymousBot"});
    e.ingest(&json!({"update_id":75,"message":m})).unwrap();
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 2);
    assert!(formatting::rendered(&replies[0]).contains("用户名 / Username：未设置 / Not set"));
    let anonymous = formatting::rendered(&replies[1]);
    assert!(anonymous.contains("无法获取个人身份"));
    assert!(anonymous.contains(&format!("身份 ID / Identity ID：{CHAT}")));
    assert!(!anonymous.contains("1087968824"));
    assert!(!anonymous.contains("用户 ID"));
    assert!(e.services.calls("getChatMember").is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn public_commands_ignore_other_bots_channels_and_edits() {
    let (e, _) = setup(None);
    let mut m = message(76);
    m["text"] = json!("/ping@AnotherBot");
    e.ingest(&json!({"update_id":76,"message":m})).unwrap();
    m["chat"] = json!({"id":USER,"type":"private"});
    e.ingest(&json!({"update_id":77,"message":m})).unwrap();
    m["text"] = json!("/whoami");
    m["chat"] = json!({"id":-1009876543210_i64,"type":"channel"});
    e.ingest(&json!({"update_id":78,"message":m})).unwrap();
    m["chat"] = json!({"id":CHAT,"type":"supergroup"});
    e.ingest(&json!({"update_id":79,"edited_message":m}))
        .unwrap();
    m["from"]["is_bot"] = json!(true);
    e.ingest(&json!({"update_id":80,"message":m})).unwrap();
    m["chat"] = json!({"id":USER,"type":"private"});
    e.ingest(&json!({"update_id":81,"message":m})).unwrap();
    drain(&e).await;
    assert!(e.services.calls("sendMessage").is_empty());
    assert!(e.services.calls("getChatMember").is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn discovery_works_before_configuration_without_managing_the_group() {
    for configured in [vec![], vec![-1009876543210]] {
        for kind in ["group", "supergroup"] {
            let (mut e, _) = setup(Some(0.99));
            Arc::get_mut(&mut e.config).unwrap().chats = configured.clone();
            // Existing data must not enable moderation in an unconfigured group.
            e.joined(CHAT, USER, NOW, true).unwrap();
            e.ingest(&json!({"update_id":900,"chat_join_request":join(NOW)}))
                .unwrap();
            e.ingest(&json!({"update_id":901,"message":message(901)}))
                .unwrap();
            for (id, text) in [(902, "/ping"), (903, "/whoami@FuukiIinTestBot")] {
                let mut m = message(id);
                m["chat"]["type"] = json!(kind);
                m["text"] = json!(text);
                let update = json!({"update_id":id,"message":m});
                e.ingest(&update).unwrap();
                e.ingest(&update).unwrap();
            }
            drain(&e).await;
            let replies = e.services.calls("sendMessage");
            assert_eq!(replies.len(), 2);
            assert_eq!(formatting::rendered(&replies[0]), "Pong! 🏓");
            assert_eq!(replies[1]["chat_id"], CHAT);
            assert!(
                formatting::rendered(&replies[1]).contains(&format!("聊天 ID / Chat ID：{CHAT}"))
            );
            assert_eq!(e.services.calls.lock().unwrap().len(), 2);
            assert!(e.services.evidence.lock().unwrap().is_empty());
            assert!(cases(&e).is_empty());
            assert!(
                e.store
                    .list::<Session>(CHAT, "session:", None, 0, 100)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                e.store
                    .get::<Member>(CHAT, &format!("member:{USER}"))
                    .unwrap()
                    .unwrap()
                    .first_message,
                None
            );
            assert_eq!(e.config.chats, configured);
        }
    }
}

#[tokio::test]
async fn clearing_groups_stops_already_queued_moderation() {
    for (next, probability) in [("classify", 0.99), ("enforce", 0.99), ("notify", 0.7)] {
        let (mut e, _) = setup(Some(probability));
        e.joined(CHAT, USER, NOW, true).unwrap();
        e.message(CHAT, &message(904)).unwrap();
        if next != "classify" {
            let job = e.store.claim(NOW).unwrap().unwrap();
            assert_eq!(job.kind, "classify");
            e.run(&job).await.unwrap();
        }
        e.services.calls.lock().unwrap().clear();
        e.services.evidence.lock().unwrap().clear();
        Arc::get_mut(&mut e.config).unwrap().chats.clear();
        let job = e.store.claim(NOW).unwrap().unwrap();
        assert_eq!(job.kind, next);
        e.run(&job).await.unwrap();
        drain(&e).await;
        assert!(e.services.calls.lock().unwrap().is_empty());
        assert!(e.services.evidence.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn ping_retries_transient_reply_failure_and_deduplicates_updates() {
    let (e, time) = setup(None);
    let mut m = message(82);
    m["chat"] = json!({"id":USER,"type":"private"});
    m["text"] = json!("/ping");
    let update = json!({"update_id":82,"message":m});
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("sendMessage".into(), 1);
    e.ingest(&update).unwrap();
    drain(&e).await;
    assert_eq!(e.services.calls("sendMessage").len(), 1);
    time.store(NOW + 10, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(e.services.calls("sendMessage").len(), 2);
    e.ingest(&update).unwrap();
    drain(&e).await;
    assert_eq!(e.services.calls("sendMessage").len(), 2);
}

#[tokio::test]
async fn approval_with_lost_response_recovers_and_accepts_already_sent_first_message() {
    let (e, time) = setup(Some(0.1));
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    let s = session(&e);
    e.receive_verification(&submission(&e, &s, USER)).unwrap();
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("approveChatJoinRequest".into(), 1);
    time.store(NOW + 10, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(session(&e).state, "approved");
    e.message(CHAT, &message(2)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e).len(), 1);
}
#[tokio::test]
async fn unban_cancels_delayed_classification_and_old_enforcement() {
    let (e, _) = setup(Some(1.0));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(2)).unwrap();
    e.services.member(7, json!({"status":"creator"}));
    admin::handle(&e,CHAT,&json!({"message":{"chat":{"id":7,"type":"private"},"from":{"id":7},"text":format!("/unban {CHAT} {USER}")}})).await.unwrap();
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "allowed");
    assert!(e.services.calls("banChatMember").is_empty());
    assert_eq!(
        e.services.calls("unbanChatMember")[0]["only_if_banned"],
        true
    );
}
#[test]
fn file_store_survives_reopen_and_instance_lock_is_exclusive() {
    let (mut e, _) = setup(None);
    let dir = std::env::temp_dir().join(format!("fuuki-iin-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("fuuki-iin.sqlite");
    Arc::get_mut(&mut e.config).unwrap().database = path.clone();
    {
        let store = Store::open(&path).unwrap();
        store
            .persist_updates(&[json!({"update_id":77})], NOW)
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.offset().unwrap(), 78);
    assert_eq!(store.claim(NOW).unwrap().unwrap().payload["update_id"], 77);
    let lock = fuuki_iin_bot::runtime::instance_lock(&e.config).unwrap();
    assert!(fuuki_iin_bot::runtime::instance_lock(&e.config).is_err());
    drop(lock);
    assert!(fuuki_iin_bot::runtime::instance_lock(&e.config).is_ok());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn same_second_rejoin_gets_new_cycle_and_old_case_cannot_ban_it() {
    let (e, _) = setup(Some(0.7));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    drain(&e).await;
    let old = cases(&e)[0].clone();
    e.message(CHAT, &json!({"date":NOW,"left_chat_member":{"id":USER}}))
        .unwrap();
    e.joined(CHAT, USER, NOW, true).unwrap();
    admin::decide(&e, CHAT, &old.id, true, 7).unwrap();
    drain(&e).await;
    assert!(e.services.calls("banChatMember").is_empty());
    e.message(CHAT, &message(2)).unwrap();
    drain(&e).await;
    assert_eq!(cases(&e).len(), 2);
    assert_ne!(cases(&e)[0].cycle, cases(&e)[1].cycle);
}
#[tokio::test]
async fn same_second_reapplication_replaces_submission_but_duplicate_update_does_not() {
    let (e, _) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    let old = session(&e);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    assert_eq!(old.claims.nonce, session(&e).claims.nonce);
    e.join_request(CHAT, &join(NOW), 101).unwrap();
    assert_ne!(old.claims.nonce, session(&e).claims.nonce);
    e.receive_verification(&submission(&e, &old, USER)).unwrap();
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
}
#[tokio::test]
async fn service_join_confirmation_does_not_reset_first_message() {
    let (e, _) = setup(Some(0.1));
    e.joined(CHAT, USER, NOW, false).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    e.joined(CHAT, USER, NOW + 1, true).unwrap();
    e.message(CHAT, &message(2)).unwrap();
    drain(&e).await;
    assert_eq!(e.services.evidence.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn lost_ban_response_is_reconciled_from_current_membership() {
    let (e, time) = setup(Some(1.0));
    e.joined(CHAT, USER, NOW, true).unwrap();
    e.message(CHAT, &message(1)).unwrap();
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("banChatMember".into(), 1);
    drain(&e).await;
    assert!(cases(&e)[0].deleted);
    assert!(!cases(&e)[0].banned);
    e.services.member(USER, json!({"status":"kicked"}));
    time.store(NOW + 10, Ordering::SeqCst);
    drain(&e).await;
    assert_eq!(cases(&e)[0].state, "banned");
    assert_eq!(e.services.calls("banChatMember").len(), 1);
}

#[tokio::test]
async fn cloudflare_binding_failures_never_approve_and_offer_a_new_challenge() {
    for (key, value) in [
        ("success", json!(false)),
        ("success", json!("true")),
        ("hostname", json!("other.example")),
        ("action", json!("login")),
        ("cdata", json!("other-session")),
    ] {
        let (e, _) = setup(None);
        e.ingest(&json!({"update_id":100,"chat_join_request":join(NOW)}))
            .unwrap();
        drain(&e).await;
        let s = session(&e);
        let mut verdict = json!({"success":true,"hostname":"verify.example.com","action":"join","cdata":s.claims.nonce});
        verdict[key] = value;
        *e.services.verdict.lock().unwrap() = Some(verdict);
        e.ingest(&json!({"update_id":101,"message":submission(&e,&s,USER)}))
            .unwrap();
        drain(&e).await;
        assert!(e.services.calls("approveChatJoinRequest").is_empty());
        assert_eq!(session(&e).state, "pending");
        assert!(session(&e).verification.is_none());
        let messages = e.services.calls("sendMessage");
        assert_eq!(messages.len(), 2);
        assert!(messages[1]["reply_markup"]["keyboard"][0][0]["web_app"].is_object());
        // User completes a new challenge, with a new Telegram message ID.
        *e.services.verdict.lock().unwrap() = None;
        let mut retry = submission(&e, &s, USER);
        retry["message_id"] = json!(21);
        e.receive_verification(&retry).unwrap();
        drain(&e).await;
        assert_eq!(e.services.calls("approveChatJoinRequest").len(), 1);
    }
}

#[tokio::test]
async fn siteverify_retries_after_restart_use_the_same_idempotency_key() {
    let (e, time) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    drain(&e).await;
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("siteverify".into(), 1);
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    // Claim but don't complete, as if interrupted by a process crash.
    let job = e.store.claim(NOW).unwrap().unwrap();
    assert_eq!(job.kind, "verify_captcha");
    e.store.recover(NOW).unwrap();
    drain(&e).await;
    assert_eq!(session(&e).state, "pending");
    time.store(NOW + 10, Ordering::SeqCst);
    drain(&e).await;
    let calls = e.services.calls("siteverify");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(session(&e).state, "approved");
}

#[tokio::test]
async fn siteverify_exhaustion_and_expiry_during_request_do_not_approve() {
    let (e, time) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    drain(&e).await;
    e.services
        .failures
        .lock()
        .unwrap()
        .insert("siteverify".into(), 10);
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    for delta in [0, 10, 30] {
        time.store(NOW + delta, Ordering::SeqCst);
        drain(&e).await;
    }
    assert_eq!(e.services.calls("siteverify").len(), 3);
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    assert!(session(&e).verification.is_none());
    let (e, time) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    drain(&e).await;
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    *e.services.after_verify.lock().unwrap() = Some((time, NOW + 600));
    drain(&e).await;
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    assert_eq!(session(&e).state, "expired");
    assert!(session(&e).verification.is_none());
}

#[tokio::test]
async fn verification_requires_owner_group_recent_submission_and_v2_session() {
    for variant in [
        "other_user",
        "other_group",
        "old_message",
        "v1_session",
        "bot_sender",
        "not_private",
        "too_large",
        "missing_token",
    ] {
        let (e, _) = setup(None);
        e.join_request(CHAT, &join(NOW), 100).unwrap();
        drain(&e).await;
        let mut s = session(&e);
        let mut m = submission(&e, &s, USER);
        match variant {
            "other_user" => {
                m["from"]["id"] = json!(777);
            }
            "other_group" => {
                let mut data: Value =
                    serde_json::from_str(m["web_app_data"]["data"].as_str().unwrap()).unwrap();
                data["chat"] = json!("-123");
                m["web_app_data"]["data"] = json!(data.to_string());
            }
            "old_message" => {
                m["date"] = json!(NOW - 300);
            }
            "v1_session" => {
                s.claims.v = 1;
                e.store
                    .put(CHAT, &format!("session:{}", s.claims.nonce), &s)
                    .unwrap();
            }
            "bot_sender" => {
                m["from"]["is_bot"] = json!(true);
            }
            "not_private" => {
                m["chat"]["type"] = json!("supergroup");
            }
            "too_large" => {
                m["web_app_data"]["data"] = json!("x".repeat(4097));
            }
            "missing_token" => {
                m["web_app_data"]["data"] = json!(
                    json!({"v":2,"session":s.claims.nonce,"chat":s.claims.chat,"success":true})
                        .to_string()
                );
            }
            _ => unreachable!(),
        }
        e.receive_verification(&m).unwrap();
        drain(&e).await;
        assert!(e.services.calls("siteverify").is_empty(), "{variant}");
        assert!(
            e.services.calls("approveChatJoinRequest").is_empty(),
            "{variant}"
        );
    }
}

#[tokio::test]
async fn session_rate_limit_and_newer_submission_bound_siteverify_work() {
    let (e, time) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    drain(&e).await;
    *e.services.verdict.lock().unwrap() = Some(json!({"success":false}));
    for id in 20..32 {
        let mut m = submission(&e, &session(&e), USER);
        m["message_id"] = json!(id);
        e.receive_verification(&m).unwrap();
        drain(&e).await;
    }
    assert_eq!(e.services.calls("siteverify").len(), 10);
    time.store(NOW + 61, Ordering::SeqCst);
    // Two pending submissions: only the most recent one is checked.
    for id in 32..34 {
        let mut m = submission(&e, &session(&e), USER);
        m["message_id"] = json!(id);
        e.receive_verification(&m).unwrap();
    }
    drain(&e).await;
    assert_eq!(e.services.calls("siteverify").len(), 11);
}

#[tokio::test]
async fn queued_token_expires_without_siteverify_and_is_removed_from_state() {
    let (e, time) = setup(None);
    e.join_request(CHAT, &join(NOW), 100).unwrap();
    drain(&e).await;
    e.receive_verification(&submission(&e, &session(&e), USER))
        .unwrap();
    time.store(NOW + 300, Ordering::SeqCst);
    e.store.prune(e.now()).unwrap();
    assert_eq!(session(&e).verification.as_ref().unwrap().token, "");
    drain(&e).await;
    assert!(e.services.calls("siteverify").is_empty());
    assert!(e.services.calls("approveChatJoinRequest").is_empty());
    assert!(session(&e).verification.is_none());
}

#[tokio::test]
async fn removing_managed_group_stops_persisted_verification_and_approval() {
    for after_siteverify in [false, true] {
        let (mut e, _) = setup(None);
        e.join_request(CHAT, &join(NOW), 100).unwrap();
        drain(&e).await;
        e.receive_verification(&submission(&e, &session(&e), USER))
            .unwrap();
        if after_siteverify {
            let job = e.store.claim(NOW).unwrap().unwrap();
            assert_eq!(job.kind, "verify_captcha");
            e.run(&job).await.unwrap();
            assert_eq!(session(&e).state, "verified");
        }
        Arc::get_mut(&mut e.config).unwrap().chats.clear();
        drain(&e).await;
        assert_eq!(
            e.services.calls("siteverify").len(),
            usize::from(after_siteverify)
        );
        assert!(e.services.calls("approveChatJoinRequest").is_empty());
        assert!(session(&e).verification.is_none());
    }
}
