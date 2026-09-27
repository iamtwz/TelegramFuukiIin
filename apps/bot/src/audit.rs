//! Bounded audit browsing; cursor data is presentation state, never authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Classification,
    Moderation,
    Joins,
    Settings,
    Errors,
}
impl Filter {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Classification,
        Self::Moderation,
        Self::Joins,
        Self::Settings,
        Self::Errors,
    ];
    pub fn code(self) -> &'static str {
        match self {
            Self::All => "a",
            Self::Classification => "c",
            Self::Moderation => "m",
            Self::Joins => "j",
            Self::Settings => "s",
            Self::Errors => "e",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Classification => "消息分类",
            Self::Moderation => "审核处罚",
            Self::Joins => "入群验证",
            Self::Settings => "设置变更",
            Self::Errors => "故障重试",
        }
    }
    pub fn actions(self) -> &'static [&'static str] {
        match self {
            Self::All => &[],
            Self::Classification => &["classified", "classification_failed_review"],
            Self::Moderation => &[
                "review_allow",
                "review_ban",
                "message_deleted",
                "user_banned",
                "ban_confirmed",
                "admin_protected",
                "guest_caller_skipped",
                "user_unbanned",
            ],
            Self::Joins => &[
                "member_joined",
                "join_requested",
                "join_approved",
                "captcha_verified",
                "captcha_rejected",
                "guard_mode_unsupported",
            ],
            Self::Settings => &["settings_changed", "thresholds_changed"],
            Self::Errors => &[
                "classification_failed_review",
                "captcha_rejected",
                "guard_mode_unsupported",
                "job_failed",
                "jobs_retried",
            ],
        }
    }
}

pub const PAGE_SIZE: i64 = 5;
#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    pub filter: Filter,
    pub snapshot: Option<i64>,
    pub page: u32,
}
impl Cursor {
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return Some(Self::default());
        }
        let mut parts = raw.split(':');
        let code = parts.next()?;
        let filter = Filter::ALL.into_iter().find(|f| f.code() == code)?;
        let snapshot = match parts.next()? {
            "n" => None,
            value => Some(value.parse::<i64>().ok().filter(|n| *n >= 0)?),
        };
        let page = parts
            .next()?
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 1_000_000)?;
        if parts.next().is_some() || (snapshot.is_none() && page != 0) {
            return None;
        }
        Some(Self {
            filter,
            snapshot,
            page,
        })
    }
    pub fn encode(self) -> String {
        format!(
            "{}:{}:{}",
            self.filter.code(),
            self.snapshot.map_or_else(|| "n".into(), |n| n.to_string()),
            self.page
        )
    }
}

pub struct Entry {
    pub id: i64,
    pub time: String,
    pub action: String,
    pub actor: Option<i64>,
    pub target: String,
    pub detail: String,
}
impl Entry {
    pub fn label(&self) -> &str {
        match self.action.as_str() {
            "classified" => "分类完成",
            "classification_failed_review" => "分类失败，转人工",
            "review_allow" => "人工放行",
            "review_ban" => "人工删除封禁",
            "message_deleted" => "消息已删除",
            "user_banned" => "用户已封禁",
            "ban_confirmed" => "确认已封禁",
            "admin_protected" => "管理员保护",
            "guest_caller_skipped" => "召唤者受保护或已重新入群",
            "user_unbanned" => "解除封禁",
            "member_joined" => "成员加入",
            "join_requested" => "申请入群",
            "join_approved" => "批准入群",
            "captcha_verified" => "人机验证通过",
            "captcha_rejected" => "人机验证失败",
            "guard_mode_unsupported" => "不支持的入群模式",
            "settings_changed" => "设置变更",
            "thresholds_changed" => "阈值变更",
            "job_failed" => "任务失败",
            "jobs_retried" => "重试任务",
            _ => &self.action,
        }
    }
    pub fn case_id(&self) -> Option<&str> {
        matches!(
            self.action.as_str(),
            "classified"
                | "classification_failed_review"
                | "review_allow"
                | "review_ban"
                | "message_deleted"
                | "user_banned"
                | "ban_confirmed"
                | "admin_protected"
                | "guest_caller_skipped"
        )
        .then_some(self.target.as_str())
    }
}
pub struct Page {
    pub cursor: Cursor,
    pub total: i64,
    pub pages: u32,
    pub entries: Vec<Entry>,
}
