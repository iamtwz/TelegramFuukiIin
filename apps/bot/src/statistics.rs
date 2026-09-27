//! Daily statistics use a fixed UTC+8 boundary, independent of the host timezone.
use crate::{
    admin::button,
    admin_view::AdminReply,
    api::Services,
    chat_info,
    engine::Engine,
    error::Result,
    markdown::{bold, code, escape},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const RETENTION_DAYS: i64 = 365;
pub fn day(timestamp: i64) -> i64 {
    timestamp.div_euclid(86400) + (timestamp.rem_euclid(86400) + 28800) / 86400
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}
impl Usage {
    pub fn from_response(data: &Value) -> Self {
        let usage = &data["usage"];
        let tokens = |key| {
            usage[key]
                .as_i64()
                .filter(|n| (0..=1_000_000_000).contains(n))
        };
        Self {
            input_tokens: tokens("input_tokens"),
            output_tokens: tokens("output_tokens"),
            cost_usd: usage["cost"]
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1_000_000.0).contains(n)),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub start: String,
    pub end: String,
    pub since: String,
    pub partial: bool,
    pub unavailable: bool,
    pub applicants: i64,
    pub approved: i64,
    pub messages: i64,
    pub classified: i64,
    pub spam: i64,
    pub review: i64,
    pub failed: i64,
    pub pending: i64,
    pub skipped: i64,
    pub profiles: i64,
    pub profile_spam: i64,
    pub profile_review: i64,
    pub profile_failed: i64,
    pub attempts: i64,
    pub input: i64,
    pub output: i64,
    pub input_known: i64,
    pub output_known: i64,
    pub cost: f64,
    pub cost_known: i64,
}

fn rate(numerator: i64, denominator: i64) -> String {
    if denominator == 0 {
        code("—")
    } else {
        code(&format!(
            "{:.1}%",
            numerator as f64 * 100.0 / denominator as f64
        ))
    }
}
fn amount(sum: i64, known: i64, attempts: i64) -> String {
    if attempts == 0 {
        return code("0");
    }
    if known == 0 {
        return format!("未知（{} 次未返回用量）", code(&attempts.to_string()));
    }
    if known == attempts {
        code(&sum.to_string())
    } else {
        format!(
            "已知 {}（另有 {} 次未知）",
            code(&sum.to_string()),
            code(&(attempts - known).to_string())
        )
    }
}

/// Daily view by default; dates are canonical YYYY-MM-DD or internal period cursors.
pub async fn show<S: Services>(
    e: &Engine<S>,
    chat: i64,
    reply: AdminReply,
    raw: &str,
) -> Result<()> {
    let today = day(e.now());
    let parsed = if raw.is_empty() {
        Some((1, today))
    } else if let Some((period, end)) = raw.split_once(':') {
        let span = match period {
            "d" => Some(1),
            "w" => Some(7),
            "m" => Some(30),
            _ => None,
        };
        span.zip(end.parse::<i64>().ok())
    } else {
        e.store.statistics_date(raw)?.map(|day| (1, day))
    };
    let Some((span, end)) =
        parsed.filter(|(_, end)| (today - RETENTION_DAYS + 1..=today).contains(end))
    else {
        return reply
            .send_markdown(
                e,
                &format!(
                    "日期无效，请使用最近 {} 天内的日期：{}",
                    code("365"),
                    code("/stats 群ID YYYY-MM-DD")
                ),
                json!({"inline_keyboard":[[button(chat, "返回上一级", "panel", "")]]}),
            )
            .await;
    };
    let start = (end - span + 1).max(today - RETENTION_DAYS + 1);
    let report = e.store.statistics_report(chat, start, end, e.now())?;
    let group = chat_info::markdown_label(e, chat).await?;
    let dates = if start == end {
        code(&report.start)
    } else {
        format!("{} 至 {}", code(&report.start), code(&report.end))
    };
    let people = if span == 1 {
        "人数"
    } else {
        "人次（每日去重后相加）"
    };
    let cost = if report.attempts == 0 {
        code("$0")
    } else if report.cost_known == 0 {
        "未知".into()
    } else if report.cost_known == report.attempts {
        code(&format!("${:.9}", report.cost))
    } else {
        format!(
            "已知 {}（另有 {} 次未知）",
            code(&format!("${:.9}", report.cost)),
            code(&(report.attempts - report.cost_known).to_string())
        )
    };
    let coverage = if report.partial {
        "；此区间包含启用前的未统计时段"
    } else {
        ""
    };
    let total_tokens = if report.attempts > 0 && report.input_known + report.output_known == 0 {
        "未知".into()
    } else if report.input_known < report.attempts || report.output_known < report.attempts {
        format!(
            "已知 {}（用量不完整）",
            code(&(report.input + report.output).to_string())
        )
    } else {
        code(&(report.input + report.output).to_string())
    };
    let profiles = format!(
        "{}\n送审：{} 次 · 高风险：{} 次\n中风险：{} 次 · 分类失败：{} 次",
        bold("入群资料审核"),
        code(&report.profiles.to_string()),
        code(&report.profile_spam.to_string()),
        code(&report.profile_review.to_string()),
        code(&report.profile_failed.to_string())
    );
    let text = if report.unavailable {
        format!(
            "{}\n{group}\n{dates} · {}\n\n该日期尚未启用统计，历史数据不可用。\n统计启用：{} {}",
            bold("群统计"),
            code("UTC+8"),
            code(&report.since),
            code("UTC+8")
        )
    } else {
        format!(
            "{}\n{group}\n{dates} · {}\n\n{}\n申请{people}：{}\n已通过{people}：{}\n通过率：{}\n\n{}\n纳入检测：{} 条\n成功分类：{} 条\n高风险：{} 条 · Spam 率：{}\n中风险（转人工）：{} 条\n分类失败：{} 条\n待检测：{} 条 · 跳过检测：{} 条\n\n{profiles}\n\n{}\n调用尝试：{} 次\n输入 Tokens：{}\n输出 Tokens：{}\nTokens 合计：{total_tokens}\n接口返回费用：{cost}\n\n{}\n统计启用：{} {}{coverage}。仅包含 Bot 观察到的数据。",
            bold("群统计"),
            code("UTC+8"),
            bold("入群申请"),
            code(&report.applicants.to_string()),
            code(&report.approved.to_string()),
            rate(report.approved, report.applicants),
            bold("消息检测"),
            code(&report.messages.to_string()),
            code(&report.classified.to_string()),
            code(&report.spam.to_string()),
            rate(report.spam, report.classified),
            code(&report.review.to_string()),
            code(&report.failed.to_string()),
            code(&report.pending.to_string()),
            code(&report.skipped.to_string()),
            bold("模型用量（消息和入群资料，按调用日期，含重试）"),
            code(&report.attempts.to_string()),
            amount(report.input, report.input_known, report.attempts),
            amount(report.output, report.output_known, report.attempts),
            escape("通过人数归属申请日；Spam 率 = 达到案件封禁阈值的条数 / 成功分类条数。"),
            code(&report.since),
            code("UTC+8")
        )
    };
    let period = match span {
        7 => "w",
        30 => "m",
        _ => "d",
    };
    let mut navigation = vec![];
    if end - span > today - RETENTION_DAYS {
        navigation.push(button(
            chat,
            if span == 1 {
                "上一天"
            } else {
                "上一时段"
            },
            "stats",
            &format!("{period}:{}", end - span),
        ));
    }
    if end < today {
        navigation.push(button(
            chat,
            if span == 1 {
                "下一天"
            } else {
                "下一时段"
            },
            "stats",
            &format!("{period}:{}", (end + span).min(today)),
        ));
    }
    let mut rows = vec![];
    if !navigation.is_empty() {
        rows.push(json!(navigation));
    }
    rows.push(json!([
        button(chat, "今天", "stats", ""),
        button(chat, "近 7 天", "stats", &format!("w:{today}")),
        button(chat, "近 30 天", "stats", &format!("m:{today}"))
    ]));
    rows.push(json!([
        button(chat, "刷新", "stats", &format!("{period}:{end}")),
        button(chat, "返回上一级", "panel", "")
    ]));
    reply
        .send_markdown(e, &text, json!({"inline_keyboard":rows}))
        .await
}
