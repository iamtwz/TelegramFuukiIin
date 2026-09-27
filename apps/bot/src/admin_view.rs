//! Administrative callbacks replace their existing private message.
use crate::{api::Services, engine::Engine, error::Result};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug)]
pub struct AdminReply {
    pub user: i64,
    pub message: Option<i64>,
}

impl AdminReply {
    pub fn new(user: i64, message: Option<i64>) -> Self {
        Self { user, message }
    }

    pub async fn send<S: Services>(self, e: &Engine<S>, text: &str, markup: Value) -> Result<()> {
        self.send_markdown(e, &crate::markdown::escape(text), markup)
            .await
    }

    /// Replace the view with complete MarkdownV2; callers split raw evidence first.
    pub async fn send_markdown<S: Services>(
        self,
        e: &Engine<S>,
        text: &str,
        markup: Value,
    ) -> Result<()> {
        let Some(message) = self.message else {
            return e.send_markdown(self.user, text, markup).await;
        };
        let markup = if markup.is_null() {
            json!({"inline_keyboard":[]})
        } else {
            markup
        };
        e.services
            .telegram(
                "editMessageText",
                json!({
                    "chat_id":self.user,
                    "message_id":message,
                    "text":text,
                    "parse_mode":"MarkdownV2",
                    "link_preview_options":{"is_disabled":true},
                    "reply_markup":markup,
                }),
            )
            .await?;
        Ok(())
    }
}

/// Preserve raw text while limiting pages by UTF-16 code units.
/// A limit below two is raised to two so a Unicode scalar always fits intact.
pub fn text_pages(text: &str, limit: usize) -> Vec<&str> {
    let limit = limit.max(2);
    let mut pages = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (offset, character) in text.char_indices() {
        let next = character.len_utf16();
        if units + next > limit {
            pages.push(&text[start..offset]);
            start = offset;
            units = 0;
        }
        units += next;
    }
    pages.push(&text[start..]);
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_pages_preserve_unicode_and_every_character_at_message_boundaries() {
        let text = format!("{}😀中\n🚀{}", "a".repeat(3899), "证据".repeat(3000));
        let pages = text_pages(&text, 3900);
        assert_eq!(pages.concat(), text);
        assert!(pages.len() > 1);
        assert!(pages.iter().all(|page| page.encode_utf16().count() <= 3900));
        assert_eq!(pages[0], "a".repeat(3899));
        assert!(pages[1].starts_with("😀中\n🚀"));
    }

    #[test]
    fn text_pages_keep_surrogate_pairs_whole_with_small_and_empty_inputs() {
        assert_eq!(text_pages("a😀b🚀", 3), vec!["a😀", "b🚀"]);
        assert_eq!(text_pages("😀🚀", 2), vec!["😀", "🚀"]);
        assert_eq!(text_pages("😀a", 1), vec!["😀", "a"]);
        assert_eq!(text_pages("😀a", 0), vec!["😀", "a"]);
        assert_eq!(text_pages("", 3900), vec![""]);
        assert_eq!(text_pages("原文\n", 3), vec!["原文\n"]);
    }
}
