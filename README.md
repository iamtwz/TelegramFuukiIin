# Telegram 风纪委员

[![CI](https://github.com/iamtwz/TelegramFuukiIin/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/iamtwz/TelegramFuukiIin/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/iamtwz/TelegramFuukiIin/blob/main/LICENSE)
[![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)](https://github.com/iamtwz/TelegramFuukiIin/blob/main/apps/bot/Cargo.toml)
[![GitHub stars](https://img.shields.io/github/stars/iamtwz/TelegramFuukiIin?style=flat)](https://github.com/iamtwz/TelegramFuukiIin/stargazers)

[English](README.en.md)

Telegram 风纪委员是一款开源的 Telegram 群组管理机器人，帮助管理员拦截垃圾广告、管理入群申请、处理可疑发言，让日常群管理更省心。

## 功能

- **智能反垃圾**：在新成员首次发言时，结合消息内容、按钮和用户资料识别广告与恶意引流，减少人工巡查。
- **自动处理与人工把关**：默认 Spam 概率 ≥80% 删除并封禁，60%–<80% 交由管理员审批，低于 60% 放行；各群可按需调整。
- **入群人机验证**：申请者通过 Telegram 内的人机验证后即可获准入群，减少逐一处理申请的工作量。
- **频道讨论群保护**：支持未入群用户的首次评论，覆盖开放讨论区的广告风险。
- **Guest Bot 治理**：审核 Guest Bot 发出的广告，并在身份明确时一并处理召唤者。
- **多群管理与协作**：在 Telegram 私聊中管理群组、调整规则、处理待审内容；按群授权，支持超级管理员跨群查看，普通用户看不到管理入口。
- **审核有据可查**：保留送审内容快照和处理记录，支持按类型筛选与分页，方便复核和追踪。
- **运营与成本统计**：查看每日申请、通过人数、通过率、审核量、Spam 率及 Token 用量。默认入群仅做人机验证，不调用付费模型；资料审核可按需开启。
- **多语言体验**：用户回复默认中英双语，验证页支持简体中文、繁体中文和英文。

## 快速部署

需要 Docker Compose v2、Node.js 24.2+、pnpm 12.4.2，以及 Telegram Bot、OpenRouter 和 Cloudflare 账号。

```sh
git clone https://github.com/iamtwz/TelegramFuukiIin.git
cd TelegramFuukiIin
```

**1. 部署验证页**

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm exec wrangler login
pnpm exec wrangler pages project create fuuki-iin-verification --production-branch main
pnpm run deploy:verification
```

在 Cloudflare Turnstile 创建 Managed 站点，允许上述 Pages 域名。

**2. 配置 Bot**

```sh
cp .env.example .env
chmod 600 .env
```

填写 Bot Token、用户名、OpenRouter Key、Pages 地址及 Turnstile 的 Site Key / Secret Key。将 Bot 设为群管理员，授予删除消息、封禁成员、邀请用户权限，并使用需要管理员批准的邀请链接。

**3. 启动**

默认使用 GHCR 预构建镜像 `ghcr.io/iamtwz/telegramfuukiiin:latest`，支持 AMD64 和 ARM64。

```sh
docker compose pull bot
docker compose run --rm bot setup-telegram
docker compose up -d bot
```

不知道群 ID 时可先留空 `MANAGED_CHAT_IDS`，在群中发送 `/whoami@你的Bot用户名`，填写取得的群 ID 后执行 `docker compose up -d bot`。私聊 `/admin` 管理群组；频道讨论群选择“改用首次发言”。

检测内容会经 OpenRouter 发送给 Jev；普通用户后续消息、编辑消息和图片中的文字不在检测范围内。

## 文档

- [部署说明](docs/DEPLOYMENT.md)
- [管理说明](docs/ADMIN.md)
- [隐私说明](docs/PRIVACY.md)
- [参与开发](CONTRIBUTING.md)
- [MIT License](LICENSE)
