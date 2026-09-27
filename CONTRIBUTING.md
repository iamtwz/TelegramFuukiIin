# 参与开发

使用 MIT 许可证。目录与流程见 [架构说明](docs/ARCHITECTURE.md)。

## 本地检查

需要 `rust-toolchain.toml` 指定的 Rust、Node.js 24.2+ 和 pnpm 12.4.2：

```sh
cargo fmt --all --check
node scripts/check-cargo-lock.mjs
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm install --frozen-lockfile --ignore-scripts
pnpm run check
pnpm run build
pnpm run test:runtime
```

Docker 可用时执行 `node scripts/test-docker.mjs`，使用合成配置和隔离的数据卷。测试不需要生产凭据。

界面预览：`pnpm run preview:verification`（8787，Cloudflare 测试 key）；Pages 本地服务：`pnpm run dev`（8788）。正式验证码必须从 Telegram 键盘 Mini App 打开。

## 提交约定

- 保留并更新 Cargo/pnpm 锁文件；固定依赖版本，不引入未经审查的 Git 或第三方 registry 依赖，不启用安装脚本。
- 发布版本以根目录 `Cargo.toml` 的 `workspace.package.version` 为准，同步 Cargo 锁文件中的工作区包和各 npm 包版本；Bot 的版本展示读取构建版本，协议版本和数据库版本独立维护。
- 行为变更补充有实际意义的测试；测试身份、链接、消息和凭据必须为合成数据。
- 普通用户回复使用中英双语（`/ping` 仅回复英文），管理界面使用中文；验证页翻译集中在 `apps/verification/public/i18n.js`。
- Bot 消息正文使用 MarkdownV2，标题/字段标签加粗，ID、用户名、命令和数值使用等宽格式；动态内容按所在上下文转义，长证据先拆分再包装代码块，不截断已格式化的 Markdown。按钮和命令菜单保持普通文本。
- 修改验证协议时同步 Rust、TypeScript 和测试向量，保持身份绑定、有效期与重放防护。
- 不提交环境文件、数据库、日志、真实群内容或带会话参数的链接。漏洞报告见 [安全政策](SECURITY.md)。
