# 部署说明

Bot 使用 Telegram long polling，本地保存 SQLite；Cloudflare Pages 仅托管静态验证页。所有命令默认在仓库根目录执行。

## 配置

从 `.env.example` 创建 `.env`，按 `KEY=value` 填写；值中含 `$` 时用单引号包住，例如 `KEY='value$with$dollars'`。

| 变量 | 用途 |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | BotFather 提供的 Token |
| `BOT_USERNAME` | Bot 用户名，不带 @ |
| `MANAGED_CHAT_IDS` | 管理的群 ID，逗号分隔；留空时不管理任何群 |
| `SUPER_ADMIN_IDS` | 可查看全部已配置群的用户 ID，可留空；写操作仍检查群权限 |
| `OPENROUTER_API_KEY` | OpenRouter API Key |
| `JEV_MODEL` | 默认 `typesafe/jev-1.13` |
| `VERIFICATION_BASE_URL` | 正式 Pages HTTPS 地址，不带路径或查询参数 |
| `TURNSTILE_SITE_KEY` | Turnstile 公钥，Bot 将其传给网页 |
| `TURNSTILE_SECRET_KEY` | 同一站点的私钥，仅保存在 Bot |
| `LOG_LEVEL` | `info`、`verbose` 或 `debug` |
| `DATABASE_PATH` | 本机运行时的数据路径；Compose 固定使用挂载卷 |

用群内 `/whoami@你的Bot用户名` 获取聊天 ID，私聊 `/whoami` 获取个人 ID。只有配置在 `MANAGED_CHAT_IDS` 中的群会触发审核、验证和后台查询。

## Cloudflare Pages 与 Turnstile

需要 Node.js 24.2+、pnpm 12.4.2。若更改 Pages 项目名，同时修改 `apps/verification/wrangler.jsonc` 的 `name` 和创建命令。

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm run check
pnpm exec wrangler login
pnpm exec wrangler pages project create fuuki-iin-verification --production-branch main
pnpm run deploy:verification
```

部署脚本会构建并上传 `dist/verification`。也可运行 `pnpm run build` 后通过 Pages Direct Upload 上传该目录；项目已存在时跳过创建命令。Pages 不需要环境变量或 Secrets。

Turnstile 使用 Managed 站点，允许域名填写正式 Pages hostname 或自定义域名。将同一站点的两枚 key 和正式 Pages 地址填入 Bot 配置；Bot 会检查 Siteverify 返回的 hostname。不要把预览域名、测试 key 或带路径的 URL 用作生产配置。

正式页面需通过 Bot 私聊发出的**键盘 Mini App 按钮**打开。直接访问网页只会提示从 Telegram 打开；inline、菜单和 Main Mini App 不支持本项目使用的回传方式。

## Docker Compose

需要 Docker Compose v2。默认使用 `ghcr.io/iamtwz/telegramfuukiiin:latest`，支持 `linux/amd64` 和 `linux/arm64`。配置完成后：

```sh
docker compose pull bot
docker compose run --rm bot check
docker compose run --rm bot setup-telegram
docker compose up -d bot
docker compose logs -f --tail=100 bot
```

`check` 只检查配置；`setup-telegram` 设置菜单并移除已有 webhook，保留待处理更新。每个 Bot Token 只运行一个实例。Bot 必须持续在线并能访问 Telegram、OpenRouter 和 Cloudflare。

配置统一保存在仓库根目录的 `.env`，Compose 自动读取。

更新镜像运行 `docker compose up -d --pull always bot`；仅修改配置运行 `docker compose up -d bot`。需要从本地源码构建时，运行 `docker compose build bot`，再运行 `docker compose up -d --pull never bot`。Compose 会按变化重建容器；`restart` 不重新读取配置。

镜像仅在 CI 检查通过后发布：`main` 更新 `latest`，每次发布保留 `sha-<完整提交 SHA>` 标签，`v*` 版本标签另发布对应版本号。发布使用 GitHub Actions 自带的 `GITHUB_TOKEN`，无需额外配置 PAT。首次发布后，维护者需在 GitHub Packages 中将包的可见性设为 Public，才能匿名拉取。

Bot 使用非 root 用户、只读根文件系统，不暴露端口。SQLite 位于 `telegram-fuuki-iin_bot-data` 卷；不要使用 `down -v` 作为重启方式。拥有 Docker 管理权限的人可以读取容器环境，不要公开 `docker inspect` 或展开后的 `docker compose config`。

## 群组设置

将 Bot 设为管理员，授予删除消息、限制成员、邀请用户权限。管理员先私聊发送 `/start`，再打开 `/admin`。

- 普通群默认审核本次入群后的首条内容消息。
- 允许未入群评论的频道讨论群，在面板切换为“首次发言”。
- 入群验证需要“管理员批准”的邀请链接；公开入群、直接拉人和其他管理员批准不受此流程约束。
- 入群资料模型审核默认关闭。开启后，资料审核和验证码都通过才批准。

首次上线用独立测试账号确认：申请后收到按钮、验证码通过后批准、管理员能看到待审案件；对测试账号验证删帖/封禁。不要用生产群成员测试处罚。

## 备份

停止 Bot 后备份整个数据目录，包括 WAL 文件：

```sh
docker compose stop bot
umask 077
mkdir -p backups
backup_file="backups/fuuki-iin-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
docker compose run --rm --no-deps -T --entrypoint tar bot -C /var/lib/fuuki-iin -czf - . > "$backup_file"
docker compose up -d bot
```

恢复时停止 Bot，将备份解压到空的数据卷；使用同版本程序，文件须允许容器 UID/GID 10001 读写。不要混入另一个数据库的 WAL。

## 本机编译

安装 `rust-toolchain.toml` 指定的 Rust 及 C/C++ 编译工具：

```sh
cargo build --release --locked -p fuuki-iin-bot
./target/release/fuuki-iin-bot check
./target/release/fuuki-iin-bot setup-telegram
./target/release/fuuki-iin-bot run
```

可用 `--env-file /绝对路径/bot.env` 指定配置文件。持续运行模板见 [systemd](../deploy/systemd/fuuki-iin.service) 和 [launchd](../deploy/launchd/ai.fuuki-iin.bot.plist)。
