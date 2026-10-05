# 仓库代理指引

本文件适用于整个仓库。开始工作前阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 和 [架构说明](docs/ARCHITECTURE.md)，并遵循其中的开发规范。

## 提交

- 所有新提交必须使用 Conventional Commits，包括合并提交：`type(scope)!: description`。`scope` 和 `!` 可选，冒号后必须有空格和变更描述。
- 新功能使用 `feat`，修复使用 `fix`；其他常用类型及格式见 [提交约定](CONTRIBUTING.md#提交约定)。不兼容变更必须用 `!` 或 `BREAKING CHANGE: ...` 标识。
- 示例：`feat(bot): add verbose group spam checks`、`docs: require conventional commits`、`chore: merge verbose group assessments into main`。
- 保留已有未提交改动，只提交本任务涉及的变更。

## 开发与验证

- 按变更范围执行 CONTRIBUTING 中的相关检查；行为变更补充有实际意义的测试，使用合成身份、消息、链接和凭据。
- 固定依赖版本并维护 Cargo/pnpm 锁文件；不启用依赖安装脚本，不引入未经审查的 Git 或第三方 registry 依赖。
- 保持业务状态与后续任务的事务一致性、任务去重和分步执行进度；网络重试和重启恢复不得重复处罚。
- 授权依据真实 Telegram 发送者及当前权限；菜单、群名缓存、消息内容、BIO 和模型输出不构成授权。
- 普通回复中英双语（`/ping` 仅英文），管理界面中文。正文使用 MarkdownV2 并按上下文转义；先拆分原始证据，再包装代码块。按钮和命令菜单使用普通文本。
- 验证协议变更同步 Rust、TypeScript 和共享测试向量，保持身份绑定、有效期与重放防护。
- 应用版本以根 Cargo.toml 为准，同步 Cargo 锁文件中的工作区包和各 npm 包版本；用户可见变化写入 CHANGELOG.md 的“未发布”部分。
- 不提交环境文件、数据库、日志、真实群内容或带会话参数的链接；数据处理遵循 [安全政策](SECURITY.md) 和 [隐私说明](docs/PRIVACY.md)。
