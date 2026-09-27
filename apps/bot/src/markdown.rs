//! Telegram MarkdownV2 helpers. Dynamic text must be escaped for its context.
pub fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\_*[]()~`>#+-=|{}.!".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn bold(text: &str) -> String {
    format!("*{}*", escape(text))
}

fn code_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('`', "\\`")
}

pub fn code(text: &str) -> String {
    format!("`{}`", code_escape(text))
}

pub fn pre(text: &str) -> String {
    format!("```\n{}\n```", code_escape(text))
}

/// Bound ordinary text before escaping, without splitting a UTF-16 surrogate pair.
pub fn truncate(text: &str, limit: usize) -> String {
    let mut length = 0;
    text.chars()
        .take_while(|c| {
            length += c.len_utf16();
            length <= limit
        })
        .collect()
}
