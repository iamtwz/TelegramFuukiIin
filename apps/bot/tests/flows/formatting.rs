//! Test-only validator for the MarkdownV2 subset emitted by this bot.
//! This is not a general Telegram parser: it accepts escaped text, bold labels,
//! inline code, language-free fenced code blocks, and links with plain labels.
use super::*;

const RESERVED: &str = "_*[]()~`>#+-=|{}.!";

struct Markdown<'a> {
    rest: &'a str,
}

impl Markdown<'_> {
    fn take(&mut self) -> char {
        let next = self.rest.chars().next().expect("unfinished MarkdownV2");
        self.rest = &self.rest[next.len_utf8()..];
        next
    }

    fn consume(&mut self, token: &str) {
        assert!(self.rest.starts_with(token), "expected {token:?}");
        self.rest = &self.rest[token.len()..];
    }

    fn escaped_text(&mut self) -> char {
        self.consume("\\");
        let next = self.take();
        assert!(
            ('\u{1}'..='\u{7e}').contains(&next),
            "MarkdownV2 text escapes must precede an ASCII character"
        );
        next
    }

    fn plain_until(&mut self, closing: char) -> String {
        let mut out = String::new();
        loop {
            let next = self
                .rest
                .chars()
                .next()
                .expect("unclosed MarkdownV2 entity");
            if next == closing {
                self.take();
                return out;
            }
            if next == '\\' {
                out.push(self.escaped_text());
            } else {
                assert!(
                    !RESERVED.contains(next),
                    "unescaped MarkdownV2 character {next:?} in text entity"
                );
                out.push(self.take());
            }
        }
    }

    fn code(&mut self, fenced: bool) -> String {
        self.consume(if fenced { "```\n" } else { "`" });
        let mut out = String::new();
        loop {
            let next = self.rest.chars().next().expect("unclosed MarkdownV2 code");
            if next == '\\' {
                self.take();
                let escaped = self.take();
                assert!(
                    matches!(escaped, '\\' | '`'),
                    "code may only escape a backslash or backtick"
                );
                out.push(escaped);
            } else if next == '`' {
                if fenced {
                    self.consume("```");
                    assert_eq!(out.pop(), Some('\n'), "missing code block separator");
                } else {
                    self.take();
                }
                return out;
            } else {
                out.push(self.take());
            }
        }
    }

    fn link(&mut self) -> String {
        self.consume("[");
        let label = self.plain_until(']');
        assert!(!label.is_empty(), "empty link label");
        self.consume("(");
        let mut url = String::new();
        loop {
            match self.take() {
                ')' => break,
                '\\' => {
                    let escaped = self.take();
                    assert!(
                        matches!(escaped, '\\' | ')'),
                        "link URL may only escape a backslash or closing parenthesis"
                    );
                    url.push(escaped);
                }
                next => url.push(next),
            }
        }
        assert!(!url.is_empty(), "empty link URL");
        label
    }

    fn render(&mut self) -> String {
        let mut out = String::new();
        while let Some(next) = self.rest.chars().next() {
            match next {
                '\\' => out.push(self.escaped_text()),
                '*' => {
                    self.take();
                    let label = self.plain_until('*');
                    assert!(!label.is_empty(), "empty bold label");
                    out.push_str(&label);
                }
                '[' => out.push_str(&self.link()),
                '`' => out.push_str(&self.code(self.rest.starts_with("```"))),
                _ => {
                    assert!(
                        !RESERVED.contains(next),
                        "unescaped MarkdownV2 character {next:?} outside an entity"
                    );
                    out.push(self.take());
                }
            }
        }
        out
    }
}

/// Validate the wire payload and return the text Telegram displays.
pub fn rendered(body: &Value) -> String {
    assert_eq!(body["parse_mode"], "MarkdownV2");
    let mut parser = Markdown {
        rest: body["text"].as_str().expect("sendMessage text"),
    };
    let text = parser.render();
    assert!(
        text.encode_utf16().count() <= 4096,
        "Telegram display text exceeds the UTF-16 limit"
    );
    text
}

#[test]
fn validator_decodes_the_emitted_entities_and_contextual_escapes() {
    use fuuki_iin_bot::markdown::{bold, code, escape, pre};
    let special = "_*[]()~`>#+-=|{}.!\\🧪";
    let evidence = format!("{special}\nsecond line\n");
    let wire = format!(
        "{}\n{}\n{}\n{}\n[原消息](https://example.invalid/a\\)b)",
        escape(special),
        bold(special),
        code(special),
        pre(&evidence)
    );
    assert_eq!(
        rendered(&json!({"text":wire,"parse_mode":"MarkdownV2"})),
        format!("{special}\n{special}\n{special}\n{evidence}\n原消息")
    );
}

#[test]
fn validator_rejects_unescaped_characters_and_broken_entities() {
    let malformed = [
        "*unclosed",
        "`unclosed",
        "```\nunclosed",
        "```\nunescaped`backtick\n```",
        "```\ninvalid\\_escape\n```",
        "`invalid\\_escape`",
        "[label]",
        "[label](https://example.invalid",
        "[label](https://example.invalid/\\.)",
        "*unescaped.name*",
        "dangling\\",
    ];
    for wire in malformed
        .into_iter()
        .map(str::to_owned)
        .chain(RESERVED.chars().map(|c| c.to_string()))
    {
        let body = json!({"text":wire,"parse_mode":"MarkdownV2"});
        assert!(
            std::panic::catch_unwind(|| rendered(&body)).is_err(),
            "accepted malformed MarkdownV2: {wire:?}"
        );
    }
}

#[tokio::test]
async fn version_uses_the_package_version_in_code_and_preserves_topic_reply() {
    let (mut e, _) = setup(None);
    Arc::get_mut(&mut e.config).unwrap().chats.clear();
    let mut m = message(8000);
    m["text"] = json!("/version@FuukiIinTestBot");
    m["message_thread_id"] = json!(20);
    e.ingest(&json!({"update_id":8000,"message":m})).unwrap();
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0]["text"],
        format!("*版本 / Version*：`{}`", fuuki_iin_bot::VERSION)
    );
    assert_eq!(
        rendered(&replies[0]),
        format!("版本 / Version：{}", fuuki_iin_bot::VERSION)
    );
    assert_eq!(replies[0]["chat_id"], CHAT);
    assert_eq!(replies[0]["message_thread_id"], 20);
    assert_eq!(replies[0]["reply_parameters"]["message_id"], 8000);
    assert_eq!(
        replies[0]["reply_parameters"]["allow_sending_without_reply"],
        true
    );
    assert_eq!(replies[0]["link_preview_options"]["is_disabled"], true);
    assert!(e.services.calls("getChatMember").is_empty());
    assert!(e.services.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn whoami_escapes_special_names_and_formats_identity_values() {
    let (e, _) = setup(None);
    let special_name = "Synthetic _*[]()~`>#+-=|{}.!\\ 🧪";
    let mut m = message(8001);
    m["text"] = json!("/whoami");
    m["from"] = json!({
        "id":USER,
        "first_name":special_name,
        "last_name":"  Test   User  ",
        "username":"synthetic_user_42",
    });
    e.ingest(&json!({"update_id":8001,"message":m})).unwrap();
    drain(&e).await;
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 1);
    let text = rendered(&replies[0]);
    assert!(text.contains(&format!("昵称 / Name：{special_name} Test User")));
    assert!(text.contains("用户名 / Username：@synthetic_user_42"));
    assert!(text.contains(&format!("用户 ID / User ID：{USER}")));
    assert!(text.contains(&format!("聊天 ID / Chat ID：{CHAT}")));
    let wire = replies[0]["text"].as_str().unwrap();
    assert!(wire.contains(&format!("*用户 ID / User ID*：`{USER}`")));
    assert!(wire.contains(&format!("*聊天 ID / Chat ID*：`{CHAT}`")));
    assert!(wire.contains("*用户名 / Username*：`@synthetic_user_42`"));
    assert!(wire.contains("\\`"));
    assert!(wire.contains("\\\\"));
}

#[tokio::test]
async fn admin_group_names_are_escaped_in_messages_and_plain_in_buttons() {
    let (e, _) = setup(None);
    let title = "Synthetic _[群] *名* `\\🧪";
    e.services.member(7, json!({"status":"creator"}));
    e.services
        .chats
        .lock()
        .unwrap()
        .insert(CHAT, json!({"id":CHAT,"type":"supergroup","title":title}));
    admin::handle(&e, 0, &admin_message(7, "/admin"))
        .await
        .unwrap();
    admin::handle(&e, CHAT, &admin_callback(7, CHAT, "panel", ""))
        .await
        .unwrap();
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 2);
    for reply in &replies {
        assert!(rendered(reply).contains(&format!("{title} · {CHAT}")));
        assert!(
            reply["text"]
                .as_str()
                .unwrap()
                .contains(&format!("`{CHAT}`"))
        );
    }
    assert_eq!(
        replies[0]["reply_markup"]["inline_keyboard"][0][0]["text"],
        format!("{title} · {CHAT}")
    );
}

#[tokio::test]
async fn ordinary_send_escapes_plain_text_without_changing_its_display() {
    let (e, _) = setup(None);
    let plain = "Synthetic _*[]()~`>#+-=|{}.!\\ 🧪\n原样显示 / Plain text";
    e.send(USER, plain, Value::Null).await.unwrap();
    let replies = e.services.calls("sendMessage");
    assert_eq!(replies.len(), 1);
    assert_eq!(rendered(&replies[0]), plain);
    let wire = replies[0]["text"].as_str().unwrap();
    assert_ne!(wire, plain);
    assert!(wire.contains("\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!\\\\"));

    // An odd-length prefix leaves no room for the next complete emoji. Escape
    // expansion may exceed 4096 wire units; Telegram limits the displayed text.
    let prefix = format!("{}_", "\\`".repeat(300));
    let long_plain = format!("{prefix}{}", "🧪".repeat(2000));
    e.send(USER, &long_plain, Value::Null).await.unwrap();
    let replies = e.services.calls("sendMessage");
    let long_reply = replies.last().unwrap();
    let display = rendered(long_reply);
    assert_eq!(display, format!("{prefix}{}", "🧪".repeat(1649)));
    assert_eq!(display.encode_utf16().count(), 3899);
    assert!(long_reply["text"].as_str().unwrap().encode_utf16().count() > 4096);
}
