# FrankClaw vs OpenClaw Deep Audit Plan

This document tracks a component-by-component audit of FrankClaw's Rust implementation
against OpenClaw's battle-tested TypeScript codebase. The goal is to find real-world
gotchas, edge cases, and production fixes in OpenClaw that FrankClaw's basic
implementation may be missing.

Each component section lists specific findings ranked by impact. After each component
is audited, fixes are implemented, tests are added, and the section is marked done.

---

## Audit Order

1. [Telegram Channel](#1-telegram-channel)
2. [Discord Channel](#2-discord-channel)
3. [Slack Channel](#3-slack-channel)
4. [WhatsApp Channel](#4-whatsapp-channel)
5. [Signal Channel](#5-signal-channel)
6. [Model Providers](#6-model-providers)
7. [Gateway & Media](#7-gateway--media)

---

## 1. Telegram Channel

**Status:** IN PROGRESS

### Critical

- [ ] **Non-idempotent send retry safety**: Only retry pre-connect errors (ECONNREFUSED, ENOTFOUND, ENETUNREACH). Post-connect errors (socket timeout after partial send) risk duplicate messages. FrankClaw must classify network errors before retrying sends.
- [ ] **401 circuit breaker on sendChatAction**: Track consecutive 401s per-account (not per-chat). After threshold (e.g. 10), suspend all sendChatAction calls with exponential backoff. Without this, a bad bot token triggers rapid 401s and Telegram may delete the bot.
- [x] **Caption length limit (1024 chars)**: Telegram captions are capped at 1024 characters. If text exceeds this, send media without caption, then send text as a follow-up message. Current implementation may silently truncate.

### High

- [x] **HTML parse error fallback**: When sending with `parse_mode: "Markdown"`, if Telegram returns "can't parse entities", retry as plain text. Without this, messages with user-generated markdown that produces invalid formatting get dropped entirely.
- [x] **Thread-not-found DM fallback**: When sending to a DM with `message_thread_id`, if Telegram returns "message thread not found", retry WITHOUT the thread ID (but only for DMs, never for forum groups). DMs can have optional topics; this fallback is legitimate.
- [x] **Message-not-modified idempotency**: When editing a message to the same text, Telegram returns 400 "message is not modified". Treat this as success, not an error. Suppresses log spam on retries.
- [ ] **20MB file download limit**: Telegram Bot API can only download files up to 20MB via `getFile()`. Detect "file is too big" errors and do NOT retry — return a placeholder instead of crashing.
- [ ] **Media group 500ms buffer**: Telegram sends multi-media albums as separate updates with the same `media_group_id`. Buffer for ~500ms to coalesce them into a single inbound event. Without this, a 4-photo album arrives as 4 separate messages.

### Medium

- [ ] **HTML entity escaping & auto-linked filenames**: Escape `&`, `<`, `>` in outbound HTML. Also detect auto-linked filenames (e.g. `README.md` becomes a link to `http://README.md`) and wrap in `<code>` tags to suppress spurious domain previews.
- [ ] **Chat-not-found error enrichment**: When Telegram returns "chat not found", wrap the error with the chat ID that failed and hints (bot not started, removed from group, group migrated with new ID).
- [ ] **Update deduplication & watermarking**: Track highest processed update ID with in-flight pending IDs. Only advance watermark when all in-flight handlers complete. Persist watermark to survive restarts.
- [ ] **Webhook secret token validation**: Validate `X-Telegram-Bot-Api-Secret-Token` header before processing webhook updates.
- [ ] **Filename preservation**: Use `document.file_name` / `audio.file_name` / `video.file_name` when available instead of server-side paths.

### Low

- [ ] **Sticker filtering**: Only support static WEBP stickers; skip animated (TGS) and video (WEBM) stickers.
- [ ] **Reaction invalid graceful degradation**: Catch `REACTION_INVALID` errors and return a warning instead of throwing.

---

## 2. Discord Channel

**Status:** IN PROGRESS

### Critical

- [x] **HELLO stall watchdog**: After WebSocket opens, wait max 30s for HELLO opcode. Track consecutive stalls — after 3 stalls without HELLO, clear resume state and force a fresh identify. Add a 5-minute reconnect stall watchdog.
- [ ] **Resume vs fresh identify state management**: On invalid session (code 4006) or intent changes, explicitly clear `sessionId`, `resumeGatewayUrl`, and `sequence`. Trying to resume with stale state causes infinite reconnect loops.
- [x] **4014 privileged intents**: Detect disallowed intents error and exit cleanly instead of retrying forever. Also covers 4004 (auth failed), 4010-4013 fatal codes.
- [ ] **Early gateway error guard**: Gateway errors can fire before lifecycle listeners are attached. Queue early errors and drain them once listeners are ready.

### High

- [ ] **Rate limit bucket handling**: Parse both `retry_after` from response body (fractional seconds) and `Retry-After` header. Use exponential backoff with jitter (10%) to avoid thundering herd.
- [x] **DM blocked (50007) vs missing permissions (50013)**: Return specific error types with human-readable messages.
- [ ] **Forum/Media channel detection**: Forum and media channels cannot receive regular messages — must create threads. Detect channel type before sending.
- [x] **Message chunking**: 2000-char limit, respects Unicode character boundaries and prefers newline split points.

### Medium

- [ ] **Thread archive state**: Check if thread is archived before sending. Consider auto-unarchive.
- [ ] **Embed field limits**: Max 10 fields, 256-char names, 1024-char values, 4096-char description, 6000-char total. Embeds only on first chunk.
- [ ] **Thread name 100-char limit**: Truncate when deriving from message text.
- [ ] **Sticker count limit**: Max 3 stickers per message, cannot be combined with other content.
- [ ] **Attachment size per tier**: Free = 8MB, Nitro = 100MB. Make configurable per account.
- [ ] **Inbound message debouncing**: Batch rapid-fire messages from same user with timeout-based flush.

### Low

- [ ] **Interaction token 3s expiry**: Auto-acknowledge interactions within 3 seconds or respond with deferred.
- [ ] **Presence cache cap**: Cap per-account presence cache to prevent unbounded memory growth.
- [ ] **Edit content only**: Message edits can only update content (not embeds/components/files).

---

## 3. Slack Channel

**Status:** IN PROGRESS

### Critical

- [x] **Non-recoverable auth error detection**: Detect `account_inactive`, `invalid_auth`, `token_revoked`, `org_login_required`, `missing_scope`, `not_allowed_token_type`, `team_access_not_granted`. Exit immediately instead of retrying — these are permanent failures.
- [ ] **Event liveness tracking**: Track `lastEventAt` and `lastInboundAt` separately to detect "half-dead" sockets that pass health checks but silently stop delivering events.

### High

- [ ] **Mrkdwn angle-bracket token preservation**: Maintain allowlist of valid angle-bracket tokens (`@`, `#`, `!`, `mailto:`, `tel:`, `http://`, `https://`, `slack://`). Only escape disallowed brackets. Escaping order: `&` first, then `<`/`>`.
- [ ] **File upload auth handling**: Presigned upload URLs must NOT include the Authorization header. Cross-origin redirects (slack.com → CDN) require manual redirect handling with auth removal. Detect HTML login pages as auth failures.
- [ ] **Thread ts ambiguity**: When `thread_ts == ts`, message IS the thread root. Detect true replies via `parent_user_id`. Track thread participation in-memory cache (24hr, max 5000 entries).
- [ ] **Event deduplication**: Dedup by `(channelId, ts)` with 60-second TTL, max 500 entries.

### Medium

- [ ] **Slack audio MIME correction**: Slack audio clips (`subtype: "slack_audio"`) served as `video/mp4` but should be treated as `audio/*` for transcription.
- [ ] **Unfurl vs forward distinction**: Only process `is_share=true` attachments as forwarded content. Unfurled URLs (link previews) are NOT user content.
- [ ] **Markdown IR chunking**: Convert to intermediate representation, chunk at 4000 chars preserving block structure, render back to mrkdwn.
- [ ] **Missing scope fallback**: If `chat:write.customize` scope is missing, retry without custom username/icon instead of failing the entire send.
- [ ] **Debounce key scoping**: Top-level messages debounce per-message (by ts), not per-sender. DMs stay channel-scoped.
- [ ] **Bot allowlist**: Per-channel `allowBots` flag. Filter self-messages by bot_user_id comparison.

### Low

- [ ] **Forwarded attachment limit**: Only process first 8 forwarded attachments.
- [ ] **File concurrency limits**: Max 8 files, 3 concurrent downloads, 5s timeout per file.
- [ ] **Interaction payload sanitization**: Redact triggerId, responseUrl, viewHash from logs. Truncate strings to 160 chars, arrays to 64 items.

---

## 4. WhatsApp Channel

**Status:** IN PROGRESS

### Critical

- [ ] **Phone number JID device suffix**: JIDs include device suffixes like `41796666864:0@s.whatsapp.net`. Must extract only digits before the colon. Without this, phone numbers get corrupted with extra digits. (N/A for Cloud API — uses phone numbers directly.)
- [x] **Status/broadcast message filtering**: Filter non-content message types (reaction, status, system, ephemeral, etc.) to prevent spurious auto-replies. Only process text, image, video, audio, document, sticker, interactive, and button types.
- [ ] **Echo detection (self-reply loop)**: In same-phone mode, detect echoed messages by tracking recently sent text. Without this, self-phone setups have infinite reply loops.

### High

- [ ] **E.164 normalization with LID support**: Handle multiple JID formats: user JID (`@s.whatsapp.net`), LID (`@lid`), plain E.164, `whatsapp:` prefix. LID requires async lookup.
- [ ] **Offline/history message handling**: On reconnect, messages with `upsert.type === "append"` should be marked read but NOT auto-replied to. Prevents replying to week-old messages.
- [ ] **Media MIME type fallback chain**: Default audio to `audio/ogg; codecs=opus`, images to `image/jpeg`, video to `video/mp4`, stickers to `image/webp` when MIME is missing.
- [ ] **Deduplication cache**: 20-minute TTL, 5000-message limit, key by `(accountId, remoteJid, messageId)`.
- [ ] **Reconnection backoff**: Exponential with jitter (initial 2s, max 30s, factor 1.8, jitter 25%, max 12 attempts).

### Medium

- [ ] **Read receipt self-chat safety**: Skip read receipts in self-chat mode to avoid deceptive blue ticks.
- [ ] **Group policy separation from DM policy**: Three independent vectors: DM policy, group policy, group allowFrom. Don't conflate.
- [ ] **Protocol message filtering**: Discard `protocolMessage` (deletions), `callLogMessage`, etc. that have no extractable text.
- [ ] **Caption preservation**: Extract captions from `imageMessage.caption`, `videoMessage.caption`, `documentMessage.caption`.
- [ ] **Context info extraction**: Handle nested reply quotes and mentions. Iterate all message values for `contextInfo`.
- [ ] **Message timeout watchdog**: Force reconnect if no activity for ~90 seconds (zombie connection detection).
- [ ] **Text chunk limit**: Default 4000 chars, configurable. Chunk preserving formatting.
- [ ] **Media file size limit**: Default 50MB, configurable. Enforce before download.

### Low

- [ ] **HEIC image conversion**: Convert HEIC (iPhone) to JPEG before sending — WhatsApp can't display HEIC natively.
- [ ] **Identifier redaction in logs**: Redact phone numbers and JIDs from log output.
- [ ] **Group metadata caching**: Cache group subject/participants with 5-minute TTL.
- [ ] **Poll validation**: Max 12 options, single/multi-select validation.

---

## 5. Signal Channel

**Status:** IN PROGRESS

### Critical

- [x] **UUID vs phone dual identity**: Self-message echo prevention compares both sourceNumber and sourceUuid against configured_account. `is_self_message()` helper added.
- [x] **SyncMessage filtering**: Already implemented — `envelope.sync_message.is_some()` check correctly catches both populated and null syncMessage fields since serde deserializes `null` as `Some(Value::Null)`.

### High

- [x] **E.164 normalization**: `normalize_e164()` strips all non-digit chars and re-adds leading `+`. `normalize_signal_identity()` routes phone-looking values through E.164 and lowercases UUIDs.
- [ ] **Attachment base64 + size pre-check**: Check `attachment.size > maxBytes` before calling RPC. Decode base64 per-attachment with try-catch so one bad attachment doesn't block others.
- [x] **Edit message handling**: Already implemented — falls back to `editMessage.dataMessage` when `dataMessage` is absent.
- [x] **Mention rendering (Object Replacement Character)**: `replace_mention_orc()` scans for U+FFFC and replaces with `@name` (falls back to phone number, then "someone").
- [ ] **Text chunking with style preservation**: ~3072 char limit. Deferred — Signal messages rarely hit this limit in practice.

### Medium

- [x] **Group ID vs sender routing**: Already implemented — thread_id keyed by `group:{groupId}`.
- [ ] **Reaction dual target identification**: Reactions are already filtered out (reaction-only messages skipped).
- [x] **Reaction removal flag**: Covered by existing reaction filtering — all reaction-only payloads are skipped.
- [ ] **Read receipt validation**: Not applicable — FrankClaw doesn't process read receipts.
- [ ] **Typing indicator stop**: Not applicable — FrankClaw doesn't send typing indicators for Signal.
- [ ] **Daemon lifecycle**: SSE reconnect exists (5s backoff in start loop). Jitter/backoff-growth deferred.
- [x] **Attachment content-type fallback**: Handled by shared `infer_inbound_mime_type()` which falls back to `application/octet-stream`.
- [ ] **Mention gate skip optimization**: Deferred — optimization, not correctness.

### Low

- [ ] **Daemon stderr classification**: Not applicable — signal-cli REST API used via SSE, not daemon process.
- [x] **SSE multiline data**: Already implemented — `SignalSseParser` accumulates multiline `data:` fields with newline separators.
- [x] **RPC envelope validation**: Already implemented — checks for `rpc.error` field and HTTP 201 status separately.

---

## 6. Model Providers

**Status:** TODO

### Critical

- [ ] **Anthropic max_tokens mandatory**: `max_tokens` is required for Anthropic but optional for OpenAI. Missing it causes HTTP 400. Ensure FrankClaw always sets it.
- [ ] **Context overflow detection**: Error messages vary across providers. Need multi-pattern matching: OpenAI "context_length_exceeded", Anthropic "prompt is too long", Ollama may not error at all.

### High

- [ ] **Ollama context window tuning**: Ollama defaults to `num_ctx=4096`, too small. Query `/api/show` per model to get actual context window. Set `num_ctx` in request options.
- [ ] **Ollama base URL normalization**: Users may configure with `/v1` suffix. Strip it before querying native API endpoints (`/api/tags`, `/api/show`).
- [ ] **402 payment vs rate-limit disambiguation**: HTTP 402 can mean "billing" (out of credits, non-retryable) or "rate_limit" (transient spend cap, retryable). Parse error message to distinguish.
- [ ] **Timeout classification**: Different runtimes emit different error codes. Need to recognize `ETIMEDOUT`, `ECONNRESET`, `ESOCKETTIMEDOUT`, `AbortError`, reqwest-specific timeouts.
- [ ] **Thinking/reasoning block handling**: Anthropic thinking blocks are semantic (preserve in transcripts). Ollama reasoning models may emit answers in `thinking` or `reasoning` fields instead of `content` — must check all three with fallback.

### Medium

- [ ] **Tool schema compatibility**: Different providers need different tool formats. Anthropic uses `input_schema`, OpenAI uses `function.parameters`. Some proxies (Bedrock) need OpenAI format even for Anthropic models.
- [ ] **Ollama model discovery concurrency**: Batch `/api/show` calls at 8x concurrency with 3s timeout each. Sequential calls are too slow.
- [ ] **Zero-cost local providers**: Default Ollama cost to `{input: 0, output: 0}` to avoid misleading billing.
- [ ] **Failover stream error handling**: If streaming has started and provider errors, fail immediately (don't try next provider). Only failover if no stream bytes received.

### Low

- [ ] **Unsafe integer handling in JSON**: Tool argument integers above 2^53 can lose precision in JSON parsing. Not critical for Rust (handles natively) but worth validating.
- [ ] **Gemini thought tag sanitization**: Strip `<think>` tags from assistant text when proxied through OpenRouter.

---

## 7. Gateway & Media

**Status:** TODO

### Critical

- [ ] **SSRF redirect chain validation**: When downloading media, validate EACH intermediate redirect URL through SSRF checker, not just the original. Limit redirect count to prevent loops.
- [ ] **MIME type binary sniffing**: Don't trust extension or Content-Type header alone. Use magic-number sniffing. If sniffed type is "generic" (octet-stream/zip) but extension is specific (xlsx), prefer extension. SVG must NOT be in allowed image types (XSS risk).

### High

- [ ] **File size dual enforcement**: Check Content-Length header before streaming (if present). Also enforce byte limit DURING streaming (Content-Length can lie). For base64, estimate decoded size before processing.
- [ ] **WebSocket tick-based stall detection**: Use app-level tick events (not just native ping/pong) to detect silent stalls. If no tick received within 2x the interval, force-close with code 4000.
- [ ] **Config hot-reload atomicity**: Ensure config is swapped atomically (ArcSwap). In-flight requests must hold a reference to old config. Debounce file-watch reloads (~300ms).

### Medium

- [ ] **Filename path traversal hardening**: Apply `path.basename()` equivalent, then sanitize non-alphanumeric chars, collapse underscores, strip leading dots, limit to 60 chars, add UUID for uniqueness.
- [ ] **HEIC detection and conversion**: Accept HEIC/HEIF in input images but convert to JPEG before forwarding to providers that don't support it.
- [ ] **Device token rotation safety**: When close code indicates device token mismatch and no explicit credentials exist, clear persisted device token. Prevents stuck invalid-auth loops.
- [ ] **Connect challenge timeout**: Guard the challenge-response handshake with a timeout (2s default, 250ms-10s range). Validate nonce is non-empty before responding.
- [ ] **Session metadata fire-and-forget**: Record session metadata asynchronously without blocking message handling. Log errors via callback instead of swallowing them.

### Low

- [ ] **Trusted proxy header validation**: Only trust X-Forwarded-For from configured trusted proxies (loopback by default). Validate Host header to prevent injection.
- [ ] **Leading-dot stripping in filenames**: Ensure filenames like `.bashrc` have leading dots stripped during sanitization.
- [ ] **Media UUID-based deduplication**: Store as `{sanitized-name}---{uuid}.ext` to prevent overwrites while embedding original name for recovery.

---

## Execution Protocol

For each component:

1. Read the FrankClaw Rust implementation for that component
2. Compare against the OpenClaw findings listed above
3. Implement fixes for Critical and High items
4. Add regression tests for each fix
5. Run `cargo test` and `cargo clippy`
6. Git commit with descriptive message
7. Mark items as done in this document
8. Move to next component

Items marked Medium/Low are addressed as time permits or when they overlap
with Critical/High work in the same file.
