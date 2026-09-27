mod support;

use fuuki_iin_bot::{
    config::{Config, parse_env},
    logging::{LogLevel, Logger},
};
use serde_json::{Value, json};
use support::Capture;

#[test]
fn levels_control_payload_visibility_and_flush_every_record() {
    for (level, count, payload) in [
        (LogLevel::Info, 1, false),
        (LogLevel::Verbose, 3, false),
        (LogLevel::Debug, 3, true),
    ] {
        let output = Capture::default();
        let logger = Logger::with_writer(level, &[], output.clone());
        logger.io(
            "api.request",
            json!({"method":"sendMessage"}),
            &json!({"text":"可见正文", "reply_markup":{"inline_keyboard":[[{"text":"按钮"}]]}}),
        );
        logger.io(
            "api.response",
            json!({"method":"decisions"}),
            &json!({"probability":0.93}),
        );
        logger.error("api.error", json!({"error":"upstream_unavailable"}));
        assert_eq!(output.records().len(), count);
        assert_eq!(output.flushes(), count);
        assert_eq!(output.text().contains("可见正文"), payload);
        assert_eq!(output.text().contains("按钮"), payload);
        assert_eq!(output.text().contains("0.93"), payload);
        assert_eq!(output.records()[0].get("body").is_some(), payload);
    }
}

#[test]
fn credentials_and_verification_proofs_are_redacted_even_inside_context_strings() {
    let shared = "0123456789abcdef0123456789abcdef";
    // Legacy proof-like strings still stay redacted when pasted in message text.
    let ticket = format!("{}.{}", "a".repeat(80), "b".repeat(43));
    let receipt = format!("{}.{}", "c".repeat(80), "d".repeat(43));
    let output = Capture::default();
    let logger = Logger::with_writer(
        LogLevel::Debug,
        &["123456:bot-secret", "sk-openrouter-secret", shared],
        output.clone(),
    );
    let context = json!({
        "message": {"text":format!("你好 sk-openrouter-secret {shared}"), "Authorization":"Bearer unconfigured-secret"},
        "url":format!("https://verification.example.invalid/?t={ticket}"),
        "receipt_in_text":format!("({receipt}).suffix"),
        "web_app_data":{"data":"untrusted-receipt"},
        "reply_markup":{"keyboard":[[{"text":"开始验证", "web_app":{"url":"opaque-verification-url"}}]]},
        "headers":{"anything":"header-value"},
        "Turnstile_Token":"turnstile-value",
        "response":"siteverify-token",
        "session":"session-value",
        "cdata":"cdata-value",
        "idempotency_key":"idempotency-value",
        "initData":"telegram-init-data",
        "nested":[{"password":"password-value", "nonce":"nonce-value"}],
        "123456:bot-secret":"secret-in-key"
    });
    logger.io(
        "api.request",
        json!({"method":"decisions"}),
        &json!({"context":context.to_string()}),
    );
    logger.error(
        "api.error",
        json!({"error":"123456:bot-secret sk-openrouter-secret"}),
    );
    let text = output.text();
    for secret in [
        "123456:bot-secret",
        "sk-openrouter-secret",
        shared,
        &ticket,
        &receipt,
        "unconfigured-secret",
        "untrusted-receipt",
        "opaque-verification-url",
        "header-value",
        "turnstile-value",
        "siteverify-token",
        "session-value",
        "cdata-value",
        "idempotency-value",
        "telegram-init-data",
        "password-value",
        "nonce-value",
    ] {
        assert!(
            !text.contains(secret),
            "redaction failed for synthetic credential"
        );
    }
    assert!(text.contains("你好"));
    assert!(text.contains("开始验证"));
    let context: Value =
        serde_json::from_str(output.records()[0]["body"]["context"].as_str().unwrap()).unwrap();
    assert_eq!(context["message"]["Authorization"], "[REDACTED]");
    assert_eq!(
        context["url"],
        "https://verification.example.invalid/?t=[REDACTED]"
    );
}

#[test]
fn untrusted_payloads_are_bounded_without_splitting_unicode_or_leaking_secret_prefixes() {
    let output = Capture::default();
    let logger = Logger::with_writer(
        LogLevel::Debug,
        &["secret-crossing-boundary"],
        output.clone(),
    );
    let mut nested = json!("deep-leaf");
    for _ in 0..30 {
        nested = json!({"child":nested});
    }
    logger.io(
        "api.response",
        json!({}),
        &json!({
            "a_text":format!("{}secret-crossing-boundary", "中".repeat(16_380)),
            "b_deep":nested,
            "c_array":vec!["entry"; 200],
            "d_huge_key":{ "键".repeat(100_000):"value" },
            "e_many_values":vec!["文".repeat(20_000); 30],
        }),
    );
    let records = output.records();
    let body = &records[0]["body"];
    assert!(body["a_text"].as_str().unwrap().ends_with("[TRUNCATED]"));
    assert!(!output.text().contains("secret"));
    assert!(!output.text().contains("deep-leaf"));
    assert_eq!(body["c_array"].as_array().unwrap().len(), 129);
    assert!(output.text().len() < 280_000);
    assert_eq!(output.flushes(), 1);
}

#[test]
fn text_cannot_inject_extra_lines_or_terminal_escapes() {
    let output = Capture::default();
    let logger = Logger::with_writer(LogLevel::Debug, &[], output.clone());
    let text = "你好\n{\"event\":\"forged\"}\r\u{1b}[31m";
    logger.io("telegram.update", json!({}), &json!({"text":text}));
    assert_eq!(output.records().len(), 1);
    assert_eq!(output.records()[0]["body"]["text"], text);
    assert!(!output.text().contains('\u{1b}'));
}

#[test]
fn logging_failure_does_not_panic() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    Logger::with_writer(LogLevel::Debug, &[], Broken).io(
        "api.request",
        json!({}),
        &json!({"text":"hello"}),
    );
}

#[test]
fn configuration_defaults_and_explicit_overrides() {
    let mut vars = parse_env("TELEGRAM_BOT_TOKEN=123456:test\nOPENROUTER_API_KEY=test\nTURNSTILE_SITE_KEY=synthetic-sitekey\nTURNSTILE_SECRET_KEY=synthetic-turnstile-secret\nVERIFICATION_BASE_URL=https://verification.example.invalid\nBOT_USERNAME=fuuki_iin_test_bot\nMANAGED_CHAT_IDS=-1001234567890").unwrap();
    assert_eq!(Config::from_map(&vars).unwrap().log_level, LogLevel::Info);
    vars.insert("LOG_LEVEL".into(), " DEBUG ".into());
    assert_eq!(Config::from_map(&vars).unwrap().log_level, LogLevel::Debug);
    assert_eq!(LogLevel::Debug.with_flags(1, false), LogLevel::Verbose);
    assert_eq!(LogLevel::Info.with_flags(2, false), LogLevel::Debug);
    assert_eq!(LogLevel::Verbose.with_flags(0, true), LogLevel::Debug);
    assert_eq!(LogLevel::Verbose.with_flags(0, false), LogLevel::Verbose);
    vars.insert("LOG_LEVEL".into(), "typo".into());
    assert!(Config::from_map(&vars).is_err());
}
