# TelegramFuukiIin

[![CI](https://github.com/iamtwz/TelegramFuukiIin/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/iamtwz/TelegramFuukiIin/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/iamtwz/TelegramFuukiIin/blob/main/LICENSE)
[![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)](https://github.com/iamtwz/TelegramFuukiIin/blob/main/apps/bot/Cargo.toml)
[![GitHub stars](https://img.shields.io/github/stars/iamtwz/TelegramFuukiIin?style=flat)](https://github.com/iamtwz/TelegramFuukiIin/stargazers)

[简体中文](README.md)

TelegramFuukiIin (Chinese name: **Telegram 风纪委员**) is an open-source Telegram moderation bot that helps admins keep spam out, manage join requests, and review suspicious messages with less daily work.

## Features

- **Intelligent spam screening:** checks new members’ first messages alongside buttons and user profiles to identify ads and suspicious promotions, reducing manual monitoring.
- **Automation with human review:** by default, spam probability ≥80% triggers deletion and a ban; 60%–<80% goes to admin review; below 60% is allowed. Thresholds are configurable per group.
- **Human verification on entry:** applicants complete verification inside Telegram to have their join requests approved, reducing manual approvals.
- **Discussion group protection:** screens first-time commenters even when they have not joined the group, covering open channel discussions.
- **Guest Bot moderation:** screens Guest Bot ads and takes action against the caller when their identity is confirmed.
- **Multi-group administration:** manage groups, adjust rules, and review cases in a private Telegram chat. Access is granted per group, super admins can view all configured groups, and management menus stay hidden from ordinary users.
- **Traceable decisions:** review submitted content snapshots and action history, with filters and pagination to help investigate cases.
- **Activity and cost insights:** track daily applicants, approvals, approval rates, screened messages, spam rates, and token usage. Join verification makes no paid model calls by default; profile screening can be enabled when needed.
- **Verbose group results:** selected groups publish probabilities and results from existing Jev screening. Members can use `/spamcheck id 123456` to evaluate an accessible User/Bot profile, or `/spamcheck text string` to evaluate text; manual evaluations take no moderation actions.
- **Multilingual experience:** user replies are bilingual Chinese/English, and the verification page supports English and Simplified/Traditional Chinese.

## Quick start

Requires Docker Compose v2, Node.js 24.2+, pnpm 12.4.2, and Telegram Bot, OpenRouter, and Cloudflare accounts.

```sh
git clone https://github.com/iamtwz/TelegramFuukiIin.git
cd TelegramFuukiIin
```

**1. Deploy the verification page**

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm exec wrangler login
pnpm exec wrangler pages project create fuuki-iin-verification --production-branch main
pnpm run deploy:verification
```

Create a Managed Turnstile widget in Cloudflare and allow the Pages hostname.

**2. Configure the bot**

```sh
cp .env.example .env
chmod 600 .env
```

Fill in the bot token, username, OpenRouter key, Pages URL, and Turnstile site/secret keys. Make the bot a group administrator with delete, restrict, and invite permissions. Use an invite link that requires admin approval.

**3. Start**

Uses the prebuilt GHCR image `ghcr.io/iamtwz/telegramfuukiiin:latest`, available for AMD64 and ARM64.

```sh
docker compose pull bot
docker compose run --rm bot setup-telegram
docker compose up -d bot
```

If you do not know the group ID, leave `MANAGED_CHAT_IDS` empty initially. Send `/whoami@YourBotUsername` in the group, add the returned chat ID, then run `docker compose up -d bot`. Open `/admin` in private chat; for linked discussion groups, select “改用首次发言” (first-seen mode).

To publish model results, add group IDs to `VERBOSE_CHAT_IDS` (a subset of `MANAGED_CHAT_IDS`), recreate the bot container; startup updates group command menus. Verbose does not enable join profile screening automatically; see the [administration guide](docs/ADMIN.md).

Screened content is sent to Jev through OpenRouter. Subsequent ordinary messages, message edits, and text inside images are outside the screening scope.

## Documentation

- [Deployment](docs/DEPLOYMENT.md)
- [Administration](docs/ADMIN.md)
- [Privacy](docs/PRIVACY.md)
- [Contributing](CONTRIBUTING.md)
- [MIT License](LICENSE)

Detailed operational documentation is in Chinese.
