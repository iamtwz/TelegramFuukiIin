# 架构

```text
apps/bot/                       Rust Bot、业务与 SQLite
apps/verification/              Pages 静态 Mini App
packages/verification-protocol/ Rust / TypeScript 共享回传格式与测试向量
deploy/                         systemd / launchd 模板
scripts/                        构建、依赖检查、运行测试
```

Bot 使用 Tokio、reqwest/rustls、Serde 和 rusqlite，直接调用 Telegram Bot API、OpenRouter Jev 和 Turnstile Siteverify。网页使用原生 HTML/CSS/JavaScript，运行时不依赖 npm 包；构建工具为 pnpm、TypeScript 和 Wrangler。

## 消息处理

Telegram long polling → SQLite 持久化更新及 offset → 判断群与检测范围 → 建立案件、保存资料快照 → Jev 分类 → 放行、人工审批或删除封禁。

首条标记和分类任务在同一事务保存。资料按案件采集，模型重试复用同一快照；Guest 回复以群/消息 ID 去重，召唤者独立记录处罚进度。执行动作前重查管理员身份与案件状态。普通消息与成员事件的时间若领先本地，记录异常但继续审核，不自动修改系统时间。

普通首条案件另存同一真实发送者在分类、待审和处罚期间的后续普通消息 ID 与时间，范围绑定群和案件，跨话题，不保存额外正文或调用 Jev。确认封禁后逐条安排独立删除任务；封禁前发出但延迟交付的消息按已保存的截止时间补充任务。任务及进度持久化、去重，重启和删除失败不会重复封禁；执行前重查案件状态、成员周期以及目标仍被封禁且不是管理员。放行、案件过期、解除封禁、管理员保护或移除群配置会停止后续清理。

首条删除和封禁状态分别保存，后续清理结果按消息 ID 写入审计；案件的 `deleted` 字段仅表示首条消息，`banned` 不表示后续清理全部成功。失败沿用任务重试、私聊告警与 `/retry`。只能清理 Bot 实际收到并记录的消息；[Telegram `deleteMessage`](https://core.telegram.org/bots/api#deletemessage) 要求消息发送未满 48 小时，超时或权限不足会记为失败。

任务在网络调用前后持久化状态，重启恢复未完成步骤。模型最多尝试 3 次后转人工；其他任务一般最多 6 次，永久错误直接失败。单进程持有数据库实例锁；同一个 Bot Token 只应运行一个实例。

## Verbose 判断

`VERBOSE_CHAT_IDS` 是已管理群的子集。已有首条消息、Guest Bot 和已开启的入群资料审核调用 Jev 时，发送一次公开概率与结果摘要；失败无评分时显示未知，不发布 BIO 或原始证据。Verbose 配置不扩大自动检测范围。

`/spamcheck id` 和 `/spamcheck text` 只在 Verbose 群接受当前实际成员的请求，不要求管理员权限。手动判断独立保存请求与任务，保留文本空白，不消费首条标记或执行处罚；每用户 10 秒冷却、每群每分钟最多 10 次。手动请求最多保留 7 天，完成后清空原文，真实模型调用用量计入调用日。群内命令菜单仅在这些群增加 `/spamcheck`，菜单展示不代替处理时的授权检查。

## 入群验证

1. Bot 收到申请，创建绑定群、用户和申请的随机会话，10 分钟有效。
2. 私聊发送键盘 Mini App 按钮；URL 仅包含群 ID、随机会话 ID 和公开 Site Key。
3. Pages 展示 Turnstile，通过 `Telegram.WebApp.sendData` 回传 token。
4. Bot 使用真实 Telegram `from.id` 校验申请者，向 Cloudflare Siteverify 提交 token。
5. 检查 success、hostname、action、cdata、最新会话和有效期，再批准入群；开启资料审核时还须通过该审核。

Pages 无后端、无 Secret。网页参数和回传数据都不构成身份或通过凭证；转发链接不能绕过发送者绑定。验证码提交限定 5 分钟内且未来不超过 30 秒，每会话限 10 次/分钟。具体字段见 [协议说明](../packages/verification-protocol/README.md)。

## 前端与授权

验证页支持简中、繁中、英文与 Telegram 明暗主题；翻译集中在 `public/i18n.js`。语言优先级为 URL 提示、手动选择、Telegram 提示、浏览器语言、英语；语言提示不用于授权。加载失败提供重试，尊重减少动画设置。

后台只接受白名单群，查询和变更均验证实际 Telegram 身份。群名称缓存不参与授权。超级管理员名单来自本地配置，不能通过聊天命令添加；查看权限不等于处罚权限。
