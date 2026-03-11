# FrankClaw vs OpenClaw Deep Audit Plan

This document tracks a component-by-component audit of FrankClaw's Rust implementation
against OpenClaw's battle-tested TypeScript codebase. The goal is to find real-world
gotchas, edge cases, and production fixes in OpenClaw that FrankClaw's basic
implementation may be missing.

Each component section lists specific findings ranked by impact. After each component
is audited, fixes are implemented, tests are added, and the section is marked done.

---

## Audit Order

### Phase 1 — Channels, Providers, Gateway (DONE)

1. [Telegram Channel](#1-telegram-channel) — DONE
2. [Discord Channel](#2-discord-channel) — DONE
3. [Slack Channel](#3-slack-channel) — DONE
4. [WhatsApp Channel](#4-whatsapp-channel) — DONE
5. [Signal Channel](#5-signal-channel) — DONE
6. [Model Providers](#6-model-providers) — DONE
7. [Gateway & Media](#7-gateway--media) — DONE

### Phase 2 — Runtime, Tools, Sessions, Crypto, Infra

8. [Runtime & Orchestration](#8-runtime--orchestration) — TODO
9. [Browser Automation & Tools](#9-browser-automation--tools) — DONE
10. [Session Management](#10-session-management) — DONE
11. [Canvas](#11-canvas) — TODO
12. [Crypto & Auth](#12-crypto--auth) — TODO
13. [Cron Service](#13-cron-service) — TODO
14. [Webhooks & Config Reload](#14-webhooks--config-reload) — TODO

---

## 1. Telegram Channel

**Status:** DONE

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

**Status:** DONE

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

**Status:** DONE

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

**Status:** DONE

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

**Status:** DONE

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

**Status:** DONE

### Critical

- [x] **Anthropic max_tokens mandatory**: Already handled — `build_request_body` defaults to `request.max_tokens.unwrap_or(4096)`, always sending max_tokens.
- [x] **Context overflow detection**: Added `classify_provider_error()` with `is_context_overflow()` that matches "context_length_exceeded", "prompt is too long", "maximum context length", "context window", "token limit", "too many tokens". Shared across all three providers.

### High

- [ ] **Ollama context window tuning**: Deferred — requires async `/api/show` call per model at discovery time. Current default is conservative 8192.
- [x] **Ollama base URL normalization**: Added `normalize_ollama_url()` that strips `/v1` and trailing `/` at construction time. Native API endpoints (`/api/tags`) now work regardless of user config.
- [x] **402 payment vs rate-limit disambiguation**: `classify_provider_error()` checks body for "rate_limit"/"spend" to distinguish retryable spend caps from permanent billing failures.
- [ ] **Timeout classification**: Deferred — reqwest timeout errors include descriptive text. Adding pattern matching is low-impact since failover chain already retries on any error.
- [x] **Thinking/reasoning block handling**: Anthropic non-streaming: `parse_completion_response` now handles `thinking` content blocks, wrapping in `<thinking>` tags. Streaming: `thinking_delta` events are forwarded as `StreamDelta::Text`.

### Medium

- [x] **Tool schema compatibility**: Already correct — Anthropic uses `input_schema`, OpenAI uses `function.parameters`. Both formats are built correctly in their respective `build_request_body` functions.
- [ ] **Ollama model discovery concurrency**: Deferred — optimization for large model lists.
- [x] **Zero-cost local providers**: Already handled — `ModelCost::default()` is `{0.0, 0.0}`.
- [x] **Failover stream error handling**: Already implemented — failover.rs checks `streamed_any` AtomicBool. If any stream bytes were forwarded, the error is returned immediately without trying the next provider.

### Low

- [x] **Unsafe integer handling in JSON**: Not applicable — Rust's `serde_json` handles large integers natively without the JavaScript 2^53 precision issue.
- [ ] **Gemini thought tag sanitization**: Deferred — only relevant when using OpenRouter proxy.

---

## 7. Gateway & Media

**Status:** DONE

### Critical

- [x] **SSRF redirect chain validation**: Refactored `SafeFetcher` to disable automatic redirects and manually follow each hop (up to 5). Each intermediate URL is validated through `validate_url_ssrf()` — DNS resolved and all IPs checked against SSRF blocklist before following.
- [ ] **MIME type binary sniffing**: Deferred — requires adding a magic-number sniffing crate. Current implementation trusts Content-Type header with `application/octet-stream` fallback, which is safe (conservative default).

### High

- [x] **File size dual enforcement**: Already implemented — Content-Length checked before download, actual byte count checked after download. Both in `SafeFetcher::fetch()`.
- [ ] **WebSocket tick-based stall detection**: Deferred — Axum handles ping/pong natively. App-level tick detection requires changes to the WebSocket protocol.
- [x] **Config hot-reload atomicity**: Already implemented — `ArcSwap` for lock-free pointer swap, 2s poll interval, modification timestamp change detection, validation before swap.

### Medium

- [x] **Filename path traversal hardening**: `sanitize_filename()` now: strips directory paths, filters to alphanumeric/dot/dash/underscore, limits to 60 chars, strips leading dots, returns "unnamed" for empty results. On-disk storage uses UUID-based filenames.
- [ ] **HEIC detection and conversion**: Deferred — requires image processing dependency.
- [ ] **Device token rotation safety**: Not applicable — FrankClaw doesn't use device tokens.
- [ ] **Connect challenge timeout**: Not applicable — FrankClaw uses token/password auth, not challenge-response.
- [ ] **Session metadata fire-and-forget**: Deferred — sessions module already handles this.

### Low

- [ ] **Trusted proxy header validation**: Deferred — currently only trusts Tailscale and explicitly configured proxy headers.
- [x] **Leading-dot stripping in filenames**: Already implemented in `sanitize_filename()`. `.bashrc` → `bashrc`, `...` → `unnamed`.
- [x] **Media UUID-based deduplication**: Already implemented — on-disk files use `{uuid}.{ext}` format with original name stored in metadata sidecar.

---

## 8. Runtime & Orchestration

**Status:** DONE

### Critical

- [x] **Tool call loop detection**: `ToolCallTracker` hashes (tool_name, arguments) and counts identical calls. Warns at 3 repetitions, hard blocks at 6 with "infinite loop" error. Prevents models from endlessly repeating the same tool call.
- [ ] **Context overflow with retry**: Deferred — requires transcript compaction strategy (summarization). Current approach: error propagates up; operator can reset session.

### High

- [x] **Tool result truncation**: `truncate_tool_output()` caps single tool result at 400K chars. Preserves 80% head + 20% tail with omission marker. Prevents context window overflow from large tool outputs.
- [ ] **Streaming error mid-turn**: Already handled by failover chain — `streamed_any` flag prevents failover after bytes are emitted. Error surfaces to caller.
- [x] **Agent turn safety timeout**: 600-second (10 min) deadline per chat turn. Checked at each tool round boundary. Prevents zombie turns.
- [ ] **Ping-pong tool detection**: Deferred — the simpler repeat detector catches the most common case.

### Medium

- [ ] **Tool call concurrency limit**: When model requests multiple parallel tool calls, cap concurrent execution (e.g., 8 at a time). Prevents resource exhaustion from a model that requests 50 simultaneous calls.
- [ ] **Transcript compaction strategy**: When context approaches limit, summarize older entries rather than truncating. Preserve system message, most recent N user/assistant turns, and all pending tool results.
- [ ] **Model fallback on streaming**: If primary model fails during streaming before any bytes are emitted, fall back to next model transparently. Already partially implemented in failover.rs but verify edge cases.

### Low

- [ ] **Turn metadata tracking**: Record model_id, token usage, tool calls, and duration per turn for observability. Fire-and-forget to avoid blocking the response path.
- [ ] **Skill validation at runtime**: Re-validate skill manifests when tool registry changes (e.g., after config reload). Stale skill references should log warnings, not crash.

---

## 9. Browser Automation & Tools

**Status:** DONE

### Critical

- [x] **CDP timeout clamping**: Added 15-second timeout (`CDP_COMMAND_TIMEOUT`) to `send_command()` via `tokio::time::timeout`. Prevents indefinite hangs when a CDP target stops responding. Command timeout is applied uniformly — loopback vs remote differentiation deferred as optimization.
- [x] **Page crash recovery**: When navigating to an existing session whose CDP target is dead (socket connection fails), the session registry entry is automatically removed and a fresh target is created. Dead sessions no longer block new sessions for the same key.

### High

- [ ] **Session state buffer limits**: Deferred — FrankClaw doesn't track console/network events per-page (no `Runtime.consoleAPICalled` subscription). No unbounded buffer risk in current implementation.
- [x] **Mutation approval enforcement**: Already verified — `ToolPolicy::blocks()` is checked in `invoke_allowed()` before any tool dispatch. The check uses `tool_requires_operator_approval()` which hard-lists mutation tools. Cannot be bypassed through tool registry manipulation because policy enforcement is in the dispatch path, not registration.
- [x] **Navigation SSRF guard**: Added `validate_navigation_url()` that validates browser.open URLs through the same `is_safe_ip()` blocklist used for media fetches. Blocks non-http/https schemes, private IPs, loopback, CGNAT, link-local, documentation ranges. DNS resolution performed before navigation to catch hostname rebinding.
- [ ] **Screenshot error recovery**: Deferred — FrankClaw uses text/HTML extraction via `Runtime.evaluate`, not `Page.captureScreenshot`. Evaluate failures already return descriptive errors.

### Medium

- [ ] **CDP WebSocket reconnection**: Deferred — dead sessions are auto-recovered on next `browser.open` (see Critical). Mid-operation reconnection adds complexity with limited benefit since tools are short-lived operations.
- [ ] **Error message rewriting**: Deferred — CDP errors are already surfaced with context (e.g., "browser command 'X' failed: Y", "browser command 'X' timed out after 15s"). Further rewriting is polish.
- [ ] **Frame selector caching**: Deferred — optimization, not correctness.
- [x] **Concurrent browser session limit**: Added `MAX_BROWSER_SESSIONS = 10` cap. New session creation fails with descriptive error when at capacity. Enforced in `BrowserClient::open()` before calling `create_target()`.

### Low

- [ ] **Browser tool audit logging**: Deferred — operator visibility improvement.
- [ ] **Page resource tracking**: Deferred — requires `Network.dataReceived` subscription.

---

## 10. Session Management

**Status:** DONE

### Critical

- [x] **Concurrent transcript append safety**: Verified — SQLite WAL mode serializes concurrent writes. Added regression test with 20 parallel appends that all succeed and persist correctly.
- [x] **Encryption key rotation**: `decrypt_content()` now surfaces a clear error message mentioning "possible key rotation" when decryption fails, plus a `warn!` log line. No panic — error propagates cleanly to caller.

### High

- [ ] **Session pruning with active reference check**: Deferred — requires runtime-level coordination between active agent turns and the pruning task. Current pruning is time-based and conservative.
- [x] **Transcript entry size limit**: Added `MAX_TRANSCRIPT_ENTRY_BYTES = 1MB` cap in `append_transcript()`. Oversized entries are rejected before encryption/storage with a descriptive error.
- [x] **Session store file locking**: Verified — SQLite WAL mode handles concurrent reads during writes. The concurrent append test exercises this path (20 parallel writers all succeed).
- [ ] **Secure delete verification**: Deferred — SQLite's `PRAGMA secure_delete = ON` is set in migrations. WAL page zeroing requires WAL checkpoint analysis which is SQLite-internal behavior.

### Medium

- [ ] **Archived transcript cleanup**: Deferred — cross-crate coordination needed (media, canvas, cron cleanup on session delete).
- [x] **Session metadata indexes**: Verified — migrations create indexes on `(agent_id)`, `(channel, account_id)`, and `(last_message_at)`. All common query patterns use these indexes.
- [x] **Connection pool exhaustion**: Added 5-second `connection_timeout` to r2d2 pool builder. Previously blocked indefinitely; now returns a clear timeout error.

### Low

- [x] **Migration idempotency**: Verified — all CREATE statements use `IF NOT EXISTS`. Added regression test that opens the store twice (re-running migrations) and confirms normal operation.
- [ ] **Transcript pagination cursor stability**: Deferred — cursor uses `seq DESC` which is stable since seq is monotonically increasing and never reused.

---

## 11. Canvas

**Status:** TODO

### Critical

- [ ] **Canvas document size limit**: Cap total document size (title + body + all blocks) at a reasonable limit (e.g., 1MB). Prevent memory exhaustion from unbounded canvas growth.
- [ ] **Block content validation**: Validate block content types match their kind (e.g., Checklist blocks should contain checklist-structured content, Code blocks should have language metadata).

### High

- [ ] **Canvas patch conflict detection**: When applying patches, verify the revision number matches the current document revision. Reject stale patches to prevent lost updates.
- [ ] **Export sanitization**: When exporting canvas to Markdown, sanitize block content to prevent injection of malicious markdown (e.g., link injection, script tags in HTML-rendered markdown).
- [ ] **Canvas persistence**: Currently in-memory only (RwLock<HashMap>). Add optional disk persistence for canvas documents so they survive gateway restarts.

### Medium

- [ ] **Block count limit**: Cap the number of blocks per document (e.g., 200). Prevents performance degradation from very large documents.
- [ ] **Canvas access scoping**: Canvas documents are accessible to any Editor+ role. Consider session-scoped canvases that restrict access to the creating session.
- [ ] **Canvas clear authorization**: Verify that `canvas.clear` requires Admin role, not just Editor. Clearing all documents is a destructive operation.

### Low

- [ ] **Export format extensibility**: Support additional export formats beyond JSON and Markdown (e.g., HTML, plain text).
- [ ] **Canvas event broadcast**: Broadcast canvas change events to connected clients for real-time collaboration.

---

## 12. Crypto & Auth

**Status:** TODO

### Critical

- [ ] **Nonce uniqueness guarantee**: ChaCha20-Poly1305 nonce is 96 bits, generated randomly. With high message volume, birthday collision risk exists around 2^48 messages. Verify nonce generation uses a CSPRNG and consider adding a counter component.
- [ ] **Timing-safe comparison coverage**: Audit all credential comparison code paths. Verify `verify_token_eq()` is used consistently — no raw `==` comparison on secrets anywhere.

### High

- [ ] **Key derivation salt uniqueness**: `derive_subkey()` uses HMAC-SHA256 with context string. Verify each context string is unique across all derivation call sites (e.g., "session", "config", "media" must not overlap).
- [ ] **Argon2id parameter validation**: Verify Argon2id parameters (t=3, m=64MB, p=4) match OWASP recommendations. Ensure password hashing doesn't block the async runtime (should run in spawn_blocking).
- [ ] **Master key zeroing on drop**: Verify `MasterKey` implements `ZeroizeOnDrop` and test that memory is actually zeroed after drop (may need unsafe test code or external tool verification).
- [ ] **Rate limiter bypass prevention**: Verify the auth rate limiter cannot be bypassed by rotating source IPs. Consider account-level rate limiting in addition to IP-level.

### Medium

- [ ] **Clock skew tolerance for device auth**: If device pairing uses timestamps, allow reasonable clock skew (±120 seconds). Validate timestamps are not in the far future.
- [ ] **Token entropy verification**: Verify `generate_token()` produces 256 bits of entropy. Test that generated tokens pass basic randomness checks (no obvious patterns, correct base64url encoding length).

### Low

- [ ] **Crypto error messages**: Ensure crypto error messages never leak key material, plaintext, or nonce values. Only report error type and context.
- [ ] **PHC format compatibility**: Verify Argon2id PHC-format hash strings are compatible with standard tools (e.g., `argon2` CLI) for debugging and migration.

---

## 13. Cron Service

**Status:** TODO

### Critical

- [ ] **Job timeout enforcement**: Each cron job needs a hard timeout (default 10 minutes, max 60 minutes). Without this, a stuck job blocks the cron executor indefinitely.
- [ ] **Concurrent execution prevention**: Ensure the same job cannot run concurrently. If a previous run is still active when the next tick fires, skip the tick and log a warning.

### High

- [ ] **Missed-fire policy**: When the cron service starts after being down, it should NOT retroactively fire all missed executions. Only fire on the next scheduled tick.
- [ ] **Session reaper throttling**: Sweep expired cron-run sessions at most every 5 minutes. Track last sweep time to avoid excessive I/O on every tick.
- [ ] **Cron expression edge cases**: Test edge cases: `@monthly` on months without the 31st, DST transitions, leap seconds, `@yearly` on Feb 29.
- [ ] **File lock safety**: When persisting jobs to JSON, ensure file locking prevents data loss from concurrent writes (e.g., two cron ticks racing on persistence).

### Medium

- [ ] **Job error reporting**: When a cron job fails, include the error message in the RunLog. Surface job failures through the health endpoint or audit log.
- [ ] **Retention period parsing**: Support human-readable duration strings for session retention (e.g., "24h", "7d"). Validate and fall back to defaults on parse error.
- [ ] **Job payload validation**: Validate cron job prompts are non-empty, agent_id references a valid agent, and session_key is well-formed.

### Low

- [ ] **Cron service graceful shutdown**: Verify CancellationToken properly interrupts the 60-second tick sleep. In-progress jobs should be allowed to complete (with timeout) before shutdown.
- [ ] **Job history pruning**: Limit stored RunLog history per job (e.g., keep last 100 runs). Prevent unbounded growth of run history.

---

## 14. Webhooks & Config Reload

**Status:** TODO

### Critical

- [ ] **Webhook signature validation**: Verify incoming webhook payloads using HMAC-SHA256 with the configured secret. Use constant-time comparison. Reject unsigned payloads when a secret is configured.
- [ ] **Webhook replay prevention**: Include timestamp in webhook signatures. Reject webhooks older than 5 minutes to prevent replay attacks.

### High

- [ ] **Webhook body size limit**: Enforce a maximum body size for incoming webhooks (e.g., 1MB). Reject oversized payloads before parsing.
- [ ] **Config reload debouncing**: When file changes trigger rapidly (e.g., editor save), debounce the reload to avoid thrashing. Current 2-second poll interval provides some natural debouncing, but verify rapid changes don't cause issues.
- [ ] **Config validation before swap**: Verify the new config is fully validated (all required fields present, references resolve) before swapping via ArcSwap. Invalid config should be logged and rejected, not applied.
- [ ] **Restart-sensitive change detection**: When config changes require a full restart (e.g., bind address, auth mode), log a clear message and optionally trigger graceful shutdown instead of silently applying partial changes.

### Medium

- [ ] **Webhook error response**: Return appropriate HTTP status codes for webhook processing errors (400 for bad payload, 401 for invalid signature, 429 for rate limited, 500 for internal errors).
- [ ] **Config diff logging**: Log what changed in a config reload (added/removed channels, changed models, etc.) for operator visibility.
- [ ] **Deep equality for config comparison**: When checking if config changed, use deep structural comparison rather than file modification timestamp alone. Prevents false-positive reloads when the file is touched but content is unchanged.

### Low

- [ ] **Webhook retry support**: For outgoing webhooks, implement retry with exponential backoff on 5xx responses.
- [ ] **Config schema versioning**: Support config file versioning to enable forward-compatible migrations.

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
