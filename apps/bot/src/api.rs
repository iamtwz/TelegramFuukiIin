use crate::{
    error::{Error, Result},
    evidence::classify_request,
    logging::{LogLevel, Logger},
    statistics::Usage,
};
use serde_json::{Value, json};
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

#[derive(Debug)]
pub struct Classification {
    pub decision: Result<(f64, String)>,
    pub usage: Usage,
}

pub trait Services: Send + Sync {
    fn telegram(&self, method: &str, body: Value) -> impl Future<Output = Result<Value>> + Send;
    fn classify(&self, evidence: &Value) -> impl Future<Output = Result<Classification>> + Send;
    fn siteverify(
        &self,
        token: &str,
        idempotency_key: &str,
    ) -> impl Future<Output = Result<Value>> + Send;
}
pub struct Api {
    client: reqwest::Client,
    token: String,
    key: String,
    model: String,
    turnstile_secret: String,
    logger: Arc<Logger>,
}
impl Api {
    pub fn new(
        token: String,
        key: String,
        model: String,
        turnstile_secret: String,
        logger: Arc<Logger>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(12))
            .user_agent("TelegramFuukiIin/0.1.0")
            .build()
            .map_err(|_| Error::Config("http_client_init_failed"))?;
        Ok(Self {
            client,
            token,
            key,
            model,
            turnstile_secret,
            logger,
        })
    }
    async fn siteverify_at(&self, url: &str, token: &str, idempotency_key: &str) -> Result<Value> {
        let (status, data) = self.post("siteverify", url,
            json!({"secret":self.turnstile_secret,"response":token,"idempotency_key":idempotency_key}), false, 8).await?;
        if !(200..300).contains(&status) {
            return Err(Error::External {
                code: format!("turnstile_http_{status}"),
                retry_after: if status == 429 { 30 } else { 0 },
                permanent: (300..500).contains(&status) && status != 429,
            });
        }
        // Malformed responses are retryable; only a real boolean verdict is usable.
        if !data["success"].is_boolean() {
            return Err(Error::external("turnstile_invalid_response", false));
        }
        Ok(data)
    }
    async fn classify_at(&self, url: &str, evidence: &Value) -> Result<Classification> {
        let (status, data) = self
            .post(
                "decisions",
                url,
                classify_request(evidence, &self.model)?,
                true,
                8,
            )
            .await?;
        // Preserve reported usage even when HTTP status or decision validation fails.
        let usage = Usage::from_response(&data);
        let decision = if !(200..300).contains(&status) {
            Err(Error::External {
                code: format!("jev_http_{status}"),
                retry_after: if status == 429 { 30 } else { 0 },
                permanent: (400..500).contains(&status) && status != 429,
            })
        } else {
            let p = data["answers"]["spam"]["noul"]
                .as_f64()
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p));
            if data["answers"]["spam"]["type"] != "noul" || p.is_none() {
                Err(Error::external("jev_invalid_probability", false))
            } else {
                Ok((
                    p.unwrap_or_default(),
                    data["model"]
                        .as_str()
                        .unwrap_or(&self.model)
                        .chars()
                        .take(128)
                        .collect(),
                ))
            }
        };
        Ok(Classification { decision, usage })
    }
    async fn post(
        &self,
        method: &str,
        url: &str,
        body: Value,
        auth: bool,
        timeout: u64,
    ) -> Result<(u16, Value)> {
        let id = self.logger.request_id();
        let started = Instant::now();
        let api = if method == "siteverify" {
            "turnstile"
        } else if auth {
            "openrouter"
        } else {
            "telegram"
        };
        self.logger.io(
            "api.request",
            json!({"request_id":id,"api":api,"method":method}),
            &body,
        );
        let mut status = None;
        let mut transport = "unknown";
        let result: Result<(u16, Value)> = async {
            let mut request = self
                .client
                .post(url)
                .timeout(Duration::from_secs(timeout))
                .json(&body);
            if auth {
                request = request.bearer_auth(&self.key);
            }
            let mut response = request.send().await.map_err(|error| {
                transport = if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connect"
                } else {
                    "request"
                };
                Error::external("upstream_unavailable", false)
            })?;
            let code = response.status().as_u16();
            status = Some(code);
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                transport = if error.is_timeout() {
                    "timeout"
                } else {
                    "response_body"
                };
                Error::external("upstream_unavailable", false)
            })? {
                if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                    return Err(Error::external("upstream_response_too_large", true));
                }
                bytes.extend_from_slice(&chunk);
            }
            let data = serde_json::from_slice(&bytes)
                .map_err(|_| Error::external("upstream_invalid_json", false))?;
            Ok((code, data))
        }
        .await;
        let mut fields = json!({"request_id":id,"api":api,"method":method,"status":status,"elapsed_ms":started.elapsed().as_millis()});
        match &result {
            Ok((code, data)) => {
                if *code >= 400 {
                    self.logger.error("api.http_error", fields.clone());
                }
                if method == "getUpdates" && data["result"].is_array() {
                    // Log each received update in Engine::ingest, not a truncated batch.
                    fields["updates"] = json!(data["result"].as_array().map_or(0, Vec::len));
                    self.logger.event(LogLevel::Verbose, "api.response", fields);
                } else {
                    self.logger.io("api.response", fields, data);
                }
            }
            Err(error) => {
                fields["error"] = json!(error.to_string());
                fields["transport"] = json!(transport);
                self.logger.error("api.error", fields);
            }
        }
        result
    }
}
impl Services for Api {
    async fn siteverify(&self, token: &str, idempotency_key: &str) -> Result<Value> {
        self.siteverify_at(
            "https://challenges.cloudflare.com/turnstile/v0/siteverify",
            token,
            idempotency_key,
        )
        .await
    }
    async fn telegram(&self, method: &str, body: Value) -> Result<Value> {
        let (status, data) = self
            .post(
                method,
                &format!("https://api.telegram.org/bot{}/{method}", self.token),
                body,
                false,
                if method == "getUpdates" { 35 } else { 8 },
            )
            .await?;
        if (200..300).contains(&status) && data["ok"] == true {
            return Ok(data["result"].clone());
        }
        let desc = data["description"].as_str().unwrap_or("").to_lowercase();
        if (method == "deleteMessage" && desc.contains("message to delete not found"))
            || (method == "answerCallbackQuery"
                && (desc.contains("query is too old") || desc.contains("query id is invalid")))
        {
            return Ok(json!(true));
        }
        let code = data["error_code"].as_u64().unwrap_or(u64::from(status));
        Err(Error::External {
            code: format!("telegram_{method}_{code}"),
            retry_after: data["parameters"]["retry_after"]
                .as_u64()
                .unwrap_or(0)
                .min(3600),
            permanent: (400..500).contains(&code) && code != 429,
        })
    }
    async fn classify(&self, evidence: &Value) -> Result<Classification> {
        self.classify_at("https://openrouter.ai/api/alpha/decisions", evidence)
            .await
    }
}

pub fn is_admin(member: &Value) -> bool {
    matches!(member["status"].as_str(), Some("administrator" | "creator"))
}
pub fn can_moderate(member: &Value) -> bool {
    member["status"] == "creator"
        || (member["status"] == "administrator"
            && member["can_delete_messages"] == true
            && member["can_restrict_members"] == true)
}
pub fn can_manage(member: &Value) -> bool {
    member["status"] == "creator" || (can_moderate(member) && member["can_invite_users"] == true)
}
pub fn present(member: &Value) -> bool {
    matches!(
        member["status"].as_str(),
        Some("member" | "administrator" | "creator")
    ) || (member["status"] == "restricted" && member["is_member"] == true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
    };

    #[tokio::test]
    async fn decisions_report_usage_even_when_status_or_probability_is_invalid() {
        let logger = Arc::new(Logger::with_writer(LogLevel::Info, &[], std::io::sink()));
        let mut api = Api::new(
            "unused".into(),
            "synthetic-key".into(),
            "test-model".into(),
            "unused".into(),
            logger,
        )
        .unwrap();
        api.client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (status, answer, usage, valid) in [
            (
                200,
                json!({"type":"noul","noul":0.9}),
                json!({"input_tokens":476,"output_tokens":70,"cost":0.000019992}),
                true,
            ),
            (200, json!({"type":"noul","noul":0.2}), Value::Null, true),
            (
                200,
                json!({"type":"noul","noul":0.2}),
                json!({"input_tokens":0,"output_tokens":0,"cost":0}),
                true,
            ),
            (
                200,
                json!({"type":"noul","noul":1.5}),
                json!({"input_tokens":19,"output_tokens":2}),
                false,
            ),
            (
                429,
                Value::Null,
                json!({"input_tokens":8,"output_tokens":1,"cost":0}),
                false,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/decisions", listener.local_addr().unwrap());
            let data = json!({"answers":{"spam":answer},"model":"dated-test-model","usage":usage});
            let body = data.to_string();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(request["questions"]["spam"]["type"], "noul");
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            });
            let response = api
                .classify_at(&url, &json!({"message":{"text":"合成样本"}}))
                .await
                .unwrap();
            server.join().unwrap();
            assert_eq!(response.decision.is_ok(), valid);
            assert_eq!(response.usage.input_tokens, usage["input_tokens"].as_i64());
            assert_eq!(
                response.usage.output_tokens,
                usage["output_tokens"].as_i64()
            );
            assert_eq!(response.usage.cost_usd, usage["cost"].as_f64());
            if status == 429 {
                assert_eq!(response.decision.unwrap_err().retry_after(), 30);
            }
        }
        let bad = Usage::from_response(
            &json!({"usage":{"input_tokens":-1,"output_tokens":1.5,"cost":-0.2}}),
        );
        assert_eq!(bad, Usage::default());
        let huge = Usage::from_response(
            &json!({"usage":{"input_tokens":9223372036854775807i64,"output_tokens":"14","cost":1e100}}),
        );
        assert_eq!(huge, Usage::default());
    }

    #[tokio::test]
    async fn response_logs_match_requests_and_redact_bodies_without_changing_results() {
        let output = test_support::Capture::default();
        let logger = Arc::new(Logger::with_writer(
            LogLevel::Debug,
            &["synthetic-secret"],
            output.clone(),
        ));
        let mut api = Api::new(
            "synthetic-token".into(),
            "synthetic-secret".into(),
            "synthetic-model".into(),
            "synthetic-turnstile-secret".into(),
            logger,
        )
        .unwrap();
        // HTTP is allowed only for this loopback fixture; production remains HTTPS-only.
        api.client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (method, status, response) in [
            (
                "sendMessage",
                200,
                json!({"ok":true,"result":{"text":"你好 synthetic-secret"}}),
            ),
            (
                "decisions",
                200,
                json!({"probability":0.93,"token":"opaque-secret"}),
            ),
            (
                "getUpdates",
                200,
                json!({"ok":true,"result":[{"update_id":1,"message":{"text":"batch-text"}}]}),
            ),
            (
                "getUpdates",
                429,
                json!({"ok":false,"description":"synthetic-secret","parameters":{"retry_after":3}}),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!(
                "http://{}/bot-synthetic-secret",
                listener.local_addr().unwrap()
            );
            let body = response.to_string();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let result = api
                .post(
                    method,
                    &url,
                    json!({"text":"测试输入 synthetic-secret"}),
                    method == "decisions",
                    5,
                )
                .await
                .unwrap();
            server.join().unwrap();
            assert_eq!(result, (status, response));
        }
        let records = output.records();
        let requests: Vec<_> = records
            .iter()
            .filter(|r| r["event"] == "api.request")
            .collect();
        let responses: Vec<_> = records
            .iter()
            .filter(|r| r["event"] == "api.response")
            .collect();
        assert_eq!(requests.len(), 4);
        assert_eq!(responses.len(), 4);
        for (request, response) in requests.iter().zip(&responses) {
            assert_eq!(
                request["fields"]["request_id"],
                response["fields"]["request_id"]
            );
            assert!(response["fields"]["elapsed_ms"].is_number());
            assert_eq!(request["body"]["text"], "测试输入 [REDACTED]");
        }
        assert_eq!(responses[0]["body"]["result"]["text"], "你好 [REDACTED]");
        assert_eq!(responses[1]["body"]["probability"], 0.93);
        assert_eq!(responses[1]["body"]["token"], "[REDACTED]");
        assert_eq!(responses[2]["fields"]["updates"], 1);
        assert!(responses[2].get("body").is_none());
        assert_eq!(responses[3]["body"]["parameters"]["retry_after"], 3);
        assert!(
            records
                .iter()
                .any(|r| r["event"] == "api.http_error" && r["fields"]["status"] == 429)
        );
        assert!(!output.text().contains("synthetic-secret"));
        assert!(!output.text().contains("opaque-secret"));
        assert!(!output.text().contains("batch-text"));
        assert!(!output.text().contains("http://"));
        assert_eq!(output.flushes(), records.len());
    }

    #[tokio::test]
    async fn turnstile_http_contract_redirects_and_secret_redaction() {
        let output = test_support::Capture::default();
        let logger = Arc::new(Logger::with_writer(
            LogLevel::Debug,
            &["test-turnstile-secret"],
            output.clone(),
        ));
        let mut api = Api::new(
            "unused".into(),
            "unused".into(),
            "unused".into(),
            "test-turnstile-secret".into(),
            logger,
        )
        .unwrap();
        api.client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for (status, verdict, expected) in [
            (
                200,
                json!({"success":true,"hostname":"verify.example.com","action":"join","cdata":"session"}),
                None,
            ),
            (
                200,
                json!({"success":false,"error-codes":["timeout-or-duplicate"]}),
                None,
            ),
            (
                200,
                json!({"success":"true"}),
                Some(("turnstile_invalid_response", false)),
            ),
            (503, json!({}), Some(("turnstile_http_503", false))),
            (429, json!({}), Some(("turnstile_http_429", false))),
            (302, json!({}), Some(("turnstile_http_302", true))),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/siteverify", listener.local_addr().unwrap());
            let body = verdict.to_string();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut length = 0;
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    headers.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                assert!(headers.starts_with("POST /siteverify HTTP/1.1"));
                assert!(!headers.to_ascii_lowercase().contains("authorization:"));
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    request,
                    json!({"secret":"test-turnstile-secret","response":"test-turnstile-token","idempotency_key":"same-attempt-key"})
                );
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/must-not-follow\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            });
            let result = api
                .siteverify_at(&url, "test-turnstile-token", "same-attempt-key")
                .await;
            server.join().unwrap();
            if let Some((code, permanent)) = expected {
                let error = result.unwrap_err();
                assert_eq!(error.to_string(), code);
                assert_eq!(error.permanent(), permanent);
            } else {
                assert_eq!(result.unwrap(), verdict);
            }
        }
        let logs = output.text();
        for secret in [
            "test-turnstile-secret",
            "test-turnstile-token",
            "same-attempt-key",
        ] {
            assert!(!logs.contains(secret));
        }
        assert!(output.records().iter().any(|r| r["event"] == "api.request"
            && r["fields"]["api"] == "turnstile"
            && r["body"]["response"] == "[REDACTED]"));
    }
}
