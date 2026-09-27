use crate::error::{Error, Result};
use serde_json::{Value, json};
use std::{
    io::{Write, stderr},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LogLevel {
    #[default]
    Info,
    Verbose,
    Debug,
}
impl LogLevel {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "info" => Ok(Self::Info),
            "verbose" => Ok(Self::Verbose),
            "debug" => Ok(Self::Debug),
            _ => Err(Error::Config("LOG_LEVEL_must_be_info_verbose_or_debug")),
        }
    }
    pub fn with_flags(self, verbose: u8, debug: bool) -> Self {
        if debug || verbose >= 2 {
            Self::Debug
        } else if verbose == 1 {
            Self::Verbose
        } else {
            self
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Verbose => "verbose",
            Self::Debug => "debug",
        }
    }
}

/// One JSON object per line on stderr, flushed immediately. No global logger,
/// third-party HTTP tracing, request URLs, or headers are used.
pub struct Logger {
    level: LogLevel,
    secrets: Vec<String>,
    writer: Mutex<Box<dyn Write + Send>>,
    sequence: AtomicU64,
}
impl Logger {
    pub fn new(level: LogLevel, secrets: &[&str]) -> Self {
        Self::with_writer(level, secrets, stderr())
    }
    pub fn with_writer(
        level: LogLevel,
        secrets: &[&str],
        writer: impl Write + Send + 'static,
    ) -> Self {
        let mut secrets: Vec<String> = secrets
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| (*s).into())
            .collect();
        // Replace full credentials before shorter overlapping ones.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self {
            level,
            secrets,
            writer: Mutex::new(Box::new(writer)),
            sequence: AtomicU64::new(1),
        }
    }
    pub fn level(&self) -> LogLevel {
        self.level
    }
    pub fn request_id(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    pub fn event(&self, level: LogLevel, event: &str, fields: Value) {
        self.record(level, level.as_str(), event, fields, None);
    }
    pub fn error(&self, event: &str, fields: Value) {
        self.record(LogLevel::Info, "error", event, fields, None);
    }
    pub fn io(&self, event: &str, fields: Value, body: &Value) {
        self.record(LogLevel::Verbose, "verbose", event, fields, Some(body));
    }
    fn record(
        &self,
        minimum: LogLevel,
        severity: &str,
        event: &str,
        fields: Value,
        body: Option<&Value>,
    ) {
        if self.level < minimum {
            return;
        }
        let mut budget = 65_536;
        let mut record = json!({
            "time_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "level": if body.is_some() && self.level == LogLevel::Debug { "debug" } else { severity },
            "event": event,
            "fields": self.sanitize(&fields, 0, &mut budget),
        });
        if self.level == LogLevel::Debug
            && let Some(body) = body
        {
            record["body"] = self.sanitize(body, 0, &mut budget);
        }
        if let Ok(mut writer) = self.writer.lock() {
            // Broken/closed logging output must not interrupt moderation.
            let _ = writeln!(writer, "{record}");
            let _ = writer.flush();
        }
    }
    fn sanitize(&self, value: &Value, depth: usize, budget: &mut usize) -> Value {
        if depth > 24 || *budget == 0 {
            return json!("[TRUNCATED]");
        }
        *budget = budget.saturating_sub(1);
        match value {
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (index, (key, value)) in map.iter().enumerate() {
                    if index >= 128 || *budget == 0 {
                        out.insert("[TRUNCATED]".into(), Value::Bool(true));
                        break;
                    }
                    let clean_key = Self::clip(self.clean_string(key), budget);
                    let normalized: String = key
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect();
                    let hidden = matches!(
                        normalized.as_str(),
                        "token"
                            | "response"
                            | "session"
                            | "cdata"
                            | "idempotencykey"
                            | "ticket"
                            | "receipt"
                            | "nonce"
                            | "secret"
                            | "apikey"
                            | "bottoken"
                            | "telegrambottoken"
                            | "openrouterapikey"
                            | "verificationsharedsecret"
                            | "accesstoken"
                            | "refreshtoken"
                            | "turnstiletoken"
                            | "turnstilesecretkey"
                            | "authorization"
                            | "proxyauthorization"
                            | "cookie"
                            | "setcookie"
                            | "password"
                            | "initdata"
                            | "initdataunsafe"
                            | "webappdata"
                            | "webapp"
                            | "headers"
                    );
                    out.insert(
                        clean_key,
                        if hidden {
                            json!("[REDACTED]")
                        } else {
                            self.sanitize(value, depth + 1, budget)
                        },
                    );
                }
                Value::Object(out)
            }
            Value::Array(items) => {
                let mut out = Vec::new();
                for (index, item) in items.iter().enumerate() {
                    if index >= 128 || *budget == 0 {
                        out.push(json!("[TRUNCATED]"));
                        break;
                    }
                    out.push(self.sanitize(item, depth + 1, budget));
                }
                Value::Array(out)
            }
            Value::String(text) => {
                // Jev's context and admin evidence can contain JSON inside a string.
                let text = if let Ok(parsed @ (Value::Object(_) | Value::Array(_))) =
                    serde_json::from_str::<Value>(text)
                {
                    self.sanitize(&parsed, depth + 1, budget).to_string()
                } else {
                    self.clean_string(text)
                };
                json!(Self::clip(text, budget))
            }
            _ => value.clone(),
        }
    }
    fn clip(text: String, budget: &mut usize) -> String {
        let clipped: String = text.chars().take((*budget).min(16_384)).collect();
        *budget = budget.saturating_sub(clipped.chars().count());
        if clipped.len() < text.len() {
            format!("{clipped}[TRUNCATED]")
        } else {
            text
        }
    }
    fn clean_string(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.secrets {
            text = text.replace(secret, "[REDACTED]");
        }
        // Signed verification proofs must also be hidden in URLs or free text,
        // rather than relying exclusively on sensitive JSON field names.
        let mut out = String::new();
        let mut token = String::new();
        for c in text.chars().chain(std::iter::once('\0')) {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                token.push(c);
            } else {
                let candidate = token.trim_matches('.');
                let parts: Vec<_> = candidate.split('.').collect();
                let proof = parts
                    .windows(2)
                    .any(|pair| pair[0].len() >= 32 && pair[1].len() == 43);
                out.push_str(if proof { "[REDACTED]" } else { &token });
                token.clear();
                if c != '\0' {
                    out.push(c);
                }
            }
        }
        out
    }
}
