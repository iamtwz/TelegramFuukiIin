# Verification protocol

Shared Rust / TypeScript schema for the Pages Mini App and the bot. Both implementations use `fixtures/v2.json` as test vectors.

## Launch and submission

The bot creates a random UUIDv4 session in SQLite and sends a **KeyboardButton.web_app** URL:

```text
https://verification.example.invalid/verify?session=32-lowercase-hex&chat=-1001234567890&sitekey=public-key
```

The page renders Turnstile with `action=join` and `cData=session`, then calls `Telegram.WebApp.sendData` with:

```json
{"v":2,"session":"0123456789abcdef0123456789abcdef","chat":"-1001234567890","token":"turnstile-token"}
```

Exactly four fields are accepted: `v=2`, a 32-character lowercase hex session, a canonical negative chat ID within JavaScript's safe integer range, and a token of 1–2048 printable non-space ASCII characters. The UTF-8 payload must fit in 4096 bytes. Duplicate/unknown JSON fields and duplicate required URL parameters are rejected.

## Verification

The URL and submitted data are untrusted. Only the private Telegram service message's actual `from.id` identifies the applicant. Rust requires a configured group, the latest matching pending session, a 10-minute session deadline, and a fresh submission. Submission timestamps must be less than 5 minutes old and at most 30 seconds ahead of the bot; each session allows 10 submissions per minute. Telegram message IDs are deduplicated.

The bot persists the token and a random idempotency key, then calls Cloudflare Siteverify without redirects. It requires `success: true`, the configured hostname, `action: join`, and matching `cdata`; it rechecks state and deadlines after the response. Transient retries reuse the idempotency key. Tokens are cleared when completed, replaced, or expired.

Approval requires successful CAPTCHA and, when enabled, successful profile review. Only the bot approves join requests; Pages stores no secrets. `sendData` closes the Mini App and the bot reports the result. Use keyboard Mini Apps; inline/menu/main Mini Apps cannot substitute for this flow.
