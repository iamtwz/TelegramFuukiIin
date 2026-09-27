# 安全政策

请通过仓库的 [**Security → Report a vulnerability**](https://github.com/iamtwz/TelegramFuukiIin/security/advisories/new) 私下报告漏洞（若维护者已启用），或先联系维护者取得私密渠道。不要在公开 Issue 中发布真实凭据、验证链接、数据库、群内容或可直接利用的细节。报告应包含版本、合成复现步骤和影响。

## 安全边界

- Telegram、OpenRouter 和 Turnstile Secret 仅由本地 Bot 使用；Pages 只发布静态资源。
- URL、Mini App 数据、BIO、消息及模型输出均视为不可信输入。授权取自真实 Telegram 发送者和当前群权限。
- 模型评分不是确定事实；中风险和模型故障转人工。Bot 只能处理 Telegram 实际交付的消息和资料。
- 验证码只约束申请加入流程，其他管理员和公开入群入口不由本项目控制。
- Debug 日志包含个人内容；备份、磁盘加密和日志访问由部署者管理，见 [隐私说明](docs/PRIVACY.md)。

## 供应链

Rust 和 pnpm 依赖固定版本并提交锁文件。安装禁用 lifecycle scripts / pnpmfile hooks；项目脚本检查官方 registry、校验值与依赖范围。Docker 基础镜像固定摘要，GitHub Actions 固定提交，CI 运行测试与依赖审计。依赖更新应经过人工审查。

```sh
node scripts/check-cargo-lock.mjs
node scripts/check-lock.mjs
cargo install cargo-audit --version 0.22.2 --locked --no-default-features
cargo audit
pnpm audit --audit-level=high
pnpm audit signatures
```

签名及漏洞检查不能保证依赖无恶意行为。发现密钥外泄时先轮换密钥，删除 Git 中的文件不能撤销凭据。
