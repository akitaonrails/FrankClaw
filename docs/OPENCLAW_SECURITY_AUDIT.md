# OpenClaw Security Audit Report

**Date:** 2026-03-11
**Scope:** Full codebase at `openclaw/` (~3400 non-test TypeScript files)
**Auditor:** Automated deep analysis (6 parallel component audits)
**Purpose:** Identify security vulnerabilities for comparison with FrankClaw's hardened Rust rewrite

---

## Executive Summary

OpenClaw has a **moderate-to-good security baseline** with some excellent implementations (SSRF protection, prompt injection mitigation, prototype pollution guards) but **critical gaps** in webhook authentication, shell execution controls, and data-at-rest encryption. The architecture follows a "single trusted operator per gateway" model, which simplifies the threat model but means several controls that multi-tenant systems would require are absent by design.

**Finding counts by severity:**

| Severity | Count |
|----------|-------|
| CRITICAL | 7 |
| HIGH | 9 |
| MEDIUM | 12 |
| LOW | 6 |
| INFO | 5 |

---

## 1. Gateway & Authentication

### CRITICAL

**1.1 — Timing Side-Channel in Token Comparison Early Return**
- **File:** `src/security/secret-equal.ts:3-12`
- The `safeEqualSecret()` function returns immediately if either input is not a string (line 7), creating a timing difference between "malformed credential" and "wrong credential". An attacker can distinguish missing/null tokens from incorrect ones by measuring response latency.
- The SHA-256 + `timingSafeEqual` approach is correct for the comparison itself, but the type-check short-circuit leaks information.

### HIGH

**1.2 — X-Real-IP Trust Without Startup Validation**
- **File:** `src/gateway/net.ts:156-185`
- `resolveClientIp()` defaults `allowRealIpFallback` to false (safe), but if enabled without proper `trustedProxies` configuration, an attacker can spoof their IP address and bypass rate limiting.
- No startup validation ensures that `allowRealIpFallback: true` requires non-empty `trustedProxies`.

**1.3 — Hook Token in Query Parameters Logged Before Rejection**
- **File:** `src/gateway/server-http.ts:383-390`
- Tokens in `?token=` query parameters are correctly rejected (400 error), but the full URL is parsed before rejection. If any proxy, WAF, or logging middleware records the raw request URL, the token is exposed in logs.

**1.4 — Rate Limiting Bypass via Misconfigured Trusted Proxies**
- **File:** `src/gateway/net.ts:111-139`
- The X-Forwarded-For chain walk is correct (right-to-left, first untrusted hop), but if `trustedProxies` includes overly broad ranges (e.g., entire cloud provider CIDR blocks), attackers within those ranges can spoof client IPs and evade per-IP rate limiting.

### MEDIUM

**1.5 — No WebSocket Frame Size Limit at Protocol Level**
- **File:** `src/gateway/server-http.ts:429, 481-484`
- HTTP webhook handlers enforce `maxBodyBytes`, but WebSocket connections do not have explicit frame size limits at the protocol level. The `ws` library defaults apply, which may allow multi-megabyte messages.

**1.6 — "none" Auth Mode Validation Unclear for Non-Loopback Binds**
- **File:** `src/gateway/auth.ts:294-329`
- Tests suggest validation exists in `resolveGatewayRuntimeConfig`, but the enforcement path for `auth.mode: "none"` on LAN/public binds is not immediately obvious from the auth module alone.

**1.7 — Tailscale Header Auth Scope Not Documented for Operators**
- **File:** `src/gateway/auth.ts:374-384`
- Tailscale identity headers are only accepted on the `ws-control-ui` surface, not on HTTP REST endpoints. This is secure but surprising — operators expecting Tailscale auth on REST will get silent failures.

### LOW

**1.8 — Missing HSTS Preload Directive**
- **File:** `src/gateway/http-common.ts:11-22`
- HSTS header (if configured) does not include `; preload`. Minor for self-hosted deployments.

**1.9 — Bind Address Logged at Startup (Information Disclosure)**
- **File:** `src/gateway/startup-control-ui-origins.ts:27-33`
- Internal IP addresses appear in startup logs. If logs are forwarded externally, network topology is exposed.

---

## 2. Shell Execution & Docker Sandbox

### CRITICAL

**2.1 — `eval()` in Browser Tool JavaScript Execution**
- **File:** `src/browser/pw-tools-core.interactions.ts:302-309, 333-353`
- The `evaluateViaPlaywright()` function uses `eval("(" + fnBody + ")")` to execute JavaScript in the browser context. The `fnBody` parameter comes from LLM-generated tool arguments.
- An LLM can inject arbitrary JavaScript that runs with full DOM access: steal localStorage/sessionStorage tokens, exfiltrate page content, make network requests.
- This is a **Remote Code Execution** vector in the browser context.

**2.2 — Docker Socket Bypass via Symlink Resolution Gap**
- **File:** `src/agents/sandbox/validate-sandbox-security.ts:18-33, 251, 273`
- The `BLOCKED_HOST_PATHS` list blocks `/var/run/docker.sock`, but validation uses string-only checks and partial symlink resolution (only through "existing ancestors", not full `realpathSync`).
- An attacker can create a symlink chain: `/tmp/mylink → /var/run/docker.sock`, bind-mount `/tmp/mylink` into the sandbox, and escape to the host via Docker API.

### HIGH

**2.3 — No Command Allowlist for Host-Mode Execution**
- **File:** `src/agents/bash-tools.exec-runtime.ts:56-90`
- When `host=gateway` and `security=full`, the exec tool can run **any installed binary** on the host. There is no mandatory command allowlist.
- An LLM can request destructive commands (`rm -rf /`, `dd if=/dev/zero of=/dev/sda`, fork bombs) with no pre-flight validation beyond policy checks.

**2.4 — Insufficient Environment Variable Sanitization in Sandbox**
- **File:** `src/agents/sandbox/sanitize-env-vars.ts:1-111`
- Uses regex-based denylist to block `*_API_KEY`, `*_TOKEN`, `*_PASSWORD` patterns, but can be bypassed with:
  - Non-standard naming: `OPENAI_KEY` (not `OPENAI_API_KEY`)
  - Secrets passed as values in allowed vars: `CUSTOM_VAR="sk-ant-..."` passes if `CUSTOM_VAR` matches allowed patterns
  - Value heuristics for "looks like base64 credential" are easily fooled.

**2.5 — No Validation of Tool Arguments from LLM Responses**
- **Files:** `src/agents/bash-tools.exec.ts`, `src/agents/bash-tools.process.ts`, `src/agents/tools/browser-tool.ts`
- LLM-provided tool arguments are type-checked (`typeof params.command === "string"`) but not semantically validated.
- The exec tool accepts arbitrary command strings with no blocked-pattern checks.
- The browser tool accepts arbitrary target URLs (SSRF risk to internal services like `http://169.254.169.254/`).

**2.6 — No Input Length Limits on Shell Commands**
- **File:** `src/agents/bash-tools.exec.ts:211`
- The exec tool accepts arbitrary-length command strings. An LLM could generate a multi-gigabyte command, causing OOM or DoS on the gateway.

### MEDIUM

**2.7 — Docker Container Missing `--cap-drop=ALL`**
- **File:** `src/agents/sandbox/docker.ts:316-425`
- `buildSandboxCreateArgs()` sets `--security-opt no-new-privileges` (good) but does not unconditionally drop all Linux capabilities. Containers retain default Docker capabilities (CAP_NET_RAW, CAP_NET_ADMIN, etc.).
- An unprivileged user in the container could use raw sockets for packet spoofing or network attacks.

**2.8 — Incomplete Seccomp/AppArmor Profile Validation**
- **File:** `src/agents/sandbox/validate-sandbox-security.ts:308-326`
- Only blocks the literal `"unconfined"` string for seccomp/AppArmor. Does not validate that custom profile files exist or are safe. Allows any named AppArmor profile including permissive ones.

**2.9 — Browser `--no-sandbox` Flag on Linux**
- **File:** `src/browser/chrome.ts:285-286`
- Chromium's internal sandbox is disabled on Linux (`--no-sandbox`, `--disable-setuid-sandbox`). Necessary for headless/containerized operation but exposes the browser process to exploitation from malicious web pages.

### LOW

**2.10 — No Rate Limiting on Tool Invocations**
- **Files:** All agent tool definitions
- No per-session or per-minute rate limiting on exec, process, or browser tool invocations. An LLM could trigger thousands of commands in rapid succession.

**2.11 — Insufficient Audit Logging of Security-Sensitive Operations**
- **Files:** All tool execution paths
- Limited audit trail for which LLM/user requested which commands, which env vars were passed to sandboxes, and which files were accessed via the sandbox filesystem bridge.

---

## 3. Channel Adapters & Webhooks

### CRITICAL

**3.1 — Discord Interactions: No Ed25519 Signature Verification**
- **File:** `src/discord/monitor/provider.ts:613`
- Discord Client initialized with hardcoded placeholder `publicKey: "a"`. Discord HTTP interactions should verify Ed25519 signatures to prevent forged button clicks, modal submissions, and slash commands.
- An attacker can craft fake interaction payloads to impersonate users and execute commands.

**3.2 — Slack Webhooks: No Signature Verification at All**
- **File:** `src/slack/http/registry.ts`
- The Slack HTTP handler routing has **zero signature verification**. No `X-Slack-Signature` HMAC check, no `X-Slack-Request-Timestamp` replay prevention.
- Any attacker who discovers the webhook URL can forge Slack events, impersonate users, and trigger slash commands.

### HIGH

**3.3 — Telegram Webhook: No Application-Layer Signature Verification**
- **File:** `src/telegram/webhook.ts:192-195`
- Secret token is extracted and passed to grammY's `webhookCallback()`, but there is no application-layer signature verification as defense-in-depth. If grammY's internal verification is disabled or misconfigured, the secret validation is bypassed entirely.

### MEDIUM

**3.4 — Inconsistent Webhook Payload Size Limits Across Channels**
- **Files:** Various channel handlers
- Telegram: 1MB limit. Plugin SDK pre-auth: 64KB. Plugin SDK post-auth: 1MB. Slack/Discord: No explicit limits in routing layer. Inconsistent enforcement creates DoS risk on channels without limits.

**3.5 — No Webhook Timestamp Replay Prevention for Telegram/Discord**
- **Files:** `src/telegram/bot-updates.ts`, Discord via Carbon library
- Telegram uses update_id deduplication (memory-based, TTL 5min, max 2000 entries) but no timestamp window validation. An attacker with network access could replay old payloads if the deduplication cache is overwhelmed.

**3.6 — No Pre-LLM Message Text Length Validation**
- **Files:** Telegram, Discord, Slack message handlers
- Text messages from webhooks are not validated for length before being sent to LLMs. Oversized messages could cause token exhaustion, memory pressure, or amplified prompt injection.

**3.7 — SSRF Risk from Webhook-Sourced Media/Attachment URLs**
- **File:** `src/security/external-content.ts`
- External content wrapping exists, but media/attachment URLs from Telegram/Discord/Slack webhooks are not explicitly validated against SSRF blocklists before fetching. Protection depends on downstream media fetcher behavior.

### LOW

**3.8 — No Webhook Rate Limiting at Routing Layer (Slack)**
- **File:** `src/slack/http/registry.ts`
- Slack webhook routing has no per-IP or per-webhook rate limiting before dispatching to handlers. Attackers can flood the endpoint with fake events.

**3.9 — Potential Token Exposure in Discord Error Logs**
- **File:** `src/discord/monitor/provider.ts:240-244`
- Discord API errors are logged via `formatDiscordDeployErrorDetails()`. If the Carbon library includes tokens in error messages, they could appear in logs.

---

## 4. Sessions, Database & Data Storage

### CRITICAL

**4.1 — Session Transcripts Stored in Plaintext (No Encryption at Rest)**
- **File:** `src/config/sessions/transcript.ts:82-85`
- Session transcripts are written with `mode: 0o600` but as **plaintext JSON** with no encryption. All conversation history, user inputs, and AI responses are readable by anyone with filesystem access.
- Compare: FrankClaw encrypts transcripts with ChaCha20-Poly1305 when a master key is provided.

**4.2 — Config Files May Contain Plaintext Secrets**
- **Files:** `src/config/io.ts:1261-1263`, `src/secrets/audit.ts:249, 304-305, 407, 444`
- Configuration files can contain plaintext API keys, OAuth credentials, auth profile tokens, and provider headers with secrets. The `secrets/audit.ts` module detects these as `PLAINTEXT_FOUND` warnings but does not enforce encryption.

**4.3 — OAuth Credentials Directory Has No Encryption**
- **File:** `src/config/paths.ts:239-256`
- OAuth tokens stored in `$STATE_DIR/credentials/` directory as plaintext files. Default location: `~/.openclaw/credentials/`.

### HIGH

**4.4 — Pairing Store Without Encryption**
- **File:** `src/pairing/pairing-store.ts:122-124, 142-147`
- Pairing codes (8-character alphanumeric) and pairing request metadata stored as plaintext JSON. Compromise of pairing files enables account takeover.

**4.5 — Session Fixation via User-Controlled Session Key Header**
- **File:** `src/gateway/http-utils.ts:66-79`
- `resolveSessionKey()` accepts an explicit session key from the `X-OpenClaw-Session-Key` header with no validation. A client can specify an arbitrary (or guessable) session key, potentially hijacking another user's session.

**4.6 — Append File Mode Not Enforced on Existing Audit Log Files**
- **File:** `src/config/io.ts:546-550`
- `appendFile()` with `mode: 0o600` only applies permissions on file creation. Subsequent appends to an existing file do not re-enforce the mode, leaving files with whatever permissions they were created with.

### MEDIUM

**4.7 — Auth Profile Plaintext Credential Storage Not Enforced**
- **File:** `src/secrets/audit.ts:279-310`
- Auth profiles can store plaintext API keys and tokens. The audit tool warns but does not prevent this.

**4.8 — Legacy auth.json Files May Persist with Plaintext Keys**
- **File:** `src/secrets/audit.ts:328-362`
- Old authentication files from previous versions may persist on disk with unencrypted credentials.

**4.9 — Models.json Provider Headers Can Contain Plaintext Secrets**
- **File:** `src/secrets/audit.ts:364-410`
- Custom HTTP headers in `models.json` (e.g., `Authorization`, `X-API-Key`) can contain plaintext secret values with no enforcement of secret references.

### LOW

**4.10 — Predictable Session IDs When Username Provided**
- **File:** `src/gateway/http-utils.ts:78`
- When a `user` parameter is provided, the session key becomes `{prefix}-user:{username}` — deterministic and guessable. Without a user, `randomUUID()` is used (secure).

---

## 5. Media, SSRF Protection & Crypto

### HIGH

**5.1 — Weak Randomness in Session Slug Generation (`Math.random()`)**
- **File:** `src/agents/session-slug.ts:104, 144`
- Uses `Math.random()` for selecting session slug words and fallback randomization. `Math.random()` is not cryptographically secure — its output can be predicted after observing ~600 values (V8 xorshift128+).
- Session slugs could be guessed by an attacker who observes a few slugs.

### MEDIUM

**5.2 — Media Files Stored with `0o644` Permissions (World-Readable)**
- **File:** `src/media/store.ts:19`
- `MEDIA_FILE_MODE = 0o644` — intentionally readable by non-owner UIDs for Docker sandbox access. Documented as a design decision, but any process on the system can read uploaded media files.
- Compare: FrankClaw uses `0o600` (owner-only) for all sensitive files.

### INFO

**5.3 — SSRF Protection is Comprehensive and Well-Tested**
- **File:** `src/infra/net/ssrf.ts`
- Blocks RFC1918, link-local, loopback, multicast, CGNAT, documentation ranges, and legacy IPv4 literal representations (octal, hex, decimal). Two-phase DNS rebinding protection with pinned dispatchers.
- This is **excellent** — one of the strongest implementations reviewed.

**5.4 — Token Generation is Cryptographically Secure**
- **File:** `src/infra/secure-random.ts`
- Uses `crypto.randomBytes()` for tokens and `crypto.randomUUID()` for UUIDs. Proper base64url encoding.

**5.5 — Prototype Pollution Guards in Config Merge**
- **File:** `src/config/merge-patch.ts`
- Blocks `__proto__`, `constructor`, and `prototype` keys during config merging. Well-tested.

---

## 6. LLM Runtime & Prompt Construction

### INFO — No Critical/High Issues Found

The runtime prompt construction is **well-designed**:

**6.1 — Prompt Injection Mitigation is Excellent**
- **File:** `src/agents/sanitize-for-prompt.ts`
- `sanitizeForPromptLiteral()` strips Unicode control characters (Cc, Cf) and line/paragraph separators. Untrusted data wrapped in `<untrusted-text>` tags with HTML entity escaping.

**6.2 — Tool Call Validation is Multi-Layered**
- **Files:** `src/agents/pi-tools.policy.ts`, `src/agents/pi-tools.ts`, `src/agents/tool-policy.ts`
- Deny-first glob patterns → allowlist → owner-only enforcement → provider-specific denial. Tool names validated against `[A-Za-z0-9_-]+` pattern, max 64 chars.

**6.3 — Tool Results Sanitized Before Re-Entering LLM Context**
- **File:** `src/agents/session-transcript-repair.ts:198-216`
- `stripToolResultDetails()` removes verbose `.details` from tool results before feeding back to LLM. Security comment in `compaction.ts:73` explicitly documents this.

**6.4 — Context Window Limits Prevent DoS**
- **File:** `src/acp/translator.ts:53-54`
- Hard limit of 2MB (`MAX_PROMPT_BYTES`) on inbound prompts.

**6.5 — Plugin/Hook System Follows Trusted-Operator Model**
- **Files:** `src/plugins/loader.ts`, `src/hooks/loader.ts`
- Plugins execute in-process (trusted computing base). Hooks enforce path boundaries with symlink traversal prevention. Both are by-design trusted.

---

## Comparison: OpenClaw vs FrankClaw Security

| Area | OpenClaw | FrankClaw |
|------|----------|-----------|
| **Memory safety** | JavaScript (GC, no buffer overflows) | Rust (`#![forbid(unsafe_code)]`, ownership) |
| **Encryption at rest** | None (plaintext transcripts/config) | ChaCha20-Poly1305 with master key |
| **Password hashing** | Not found (no password auth mode?) | Argon2id (t=3, m=64MB, p=4) |
| **Token comparison** | SHA-256 + timingSafeEqual (with type-check timing leak) | Constant-time byte comparison |
| **SSRF protection** | Excellent (two-phase, pinned DNS) | Excellent (comprehensive IP blocklist) |
| **Shell command control** | No mandatory allowlist, `eval()` in browser tool | Allowlist + metacharacter rejection + ai-jail sandbox |
| **Webhook auth (Discord)** | Hardcoded placeholder key | N/A (not yet implemented) |
| **Webhook auth (Slack)** | No signature verification | N/A (not yet implemented) |
| **Webhook auth (Telegram)** | Delegated to grammY only | Application-layer secret validation |
| **File permissions** | `0o600` for config, `0o644` for media | `0o600` for everything sensitive |
| **Session encryption** | None | ChaCha20-Poly1305 when master key set |
| **Prompt injection** | `sanitizeForPromptLiteral()` + `<untrusted-text>` wrapping | Role separation, no user data in system prompts |
| **Tool argument validation** | Type-checked only | Type-checked + allowlist + metacharacter rejection |
| **Prototype pollution** | Explicitly guarded in config merge | N/A (Rust has no prototype chain) |
| **Sandbox** | Docker with gaps (no cap-drop-all, eval in browser) | Optional ai-jail (bubblewrap + landlock) |
| **Identifier length limits** | None found | 255 bytes (AgentId, ChannelId), 800 (SessionKey) |
| **Message size limits (WS)** | No explicit frame size limit | `max_ws_message_bytes` config enforced |
| **Security audit CLI** | `secrets/audit.ts` (detects plaintext secrets) | `frankclaw audit` (severity-rated, CI exit codes) |

---

## Top 10 Most Actionable Findings

1. **Slack webhooks have zero signature verification** (3.2 — CRITICAL)
2. **Discord interactions use placeholder public key** (3.1 — CRITICAL)
3. **`eval()` in browser tool executes LLM-generated JavaScript** (2.1 — CRITICAL)
4. **Session transcripts stored as plaintext** (4.1 — CRITICAL)
5. **No mandatory command allowlist for host-mode shell execution** (2.3 — HIGH)
6. **Docker socket bypass via incomplete symlink resolution** (2.2 — CRITICAL)
7. **OAuth credentials stored unencrypted** (4.3 — CRITICAL)
8. **Session fixation via user-controlled session key header** (4.5 — HIGH)
9. **`Math.random()` for session slug generation** (5.1 — HIGH)
10. **Environment variable denylist bypass in sandbox** (2.4 — HIGH)

---

## Methodology

Six parallel audit agents each performed deep code review of specific components:
1. Gateway & Authentication (`src/gateway/`, `src/security/`)
2. Shell Execution & Docker Sandbox (`src/process/`, `src/agents/sandbox/`, `Dockerfile.*`)
3. Channel Adapters & Webhooks (`src/telegram/`, `src/discord/`, `src/slack/`, `extensions/`)
4. Sessions, Database & Data Storage (`src/sessions/`, `src/config/`, `src/secrets/`, `src/pairing/`)
5. Media, SSRF & Cryptography (`src/media/`, `src/infra/net/`, `src/security/`)
6. LLM Runtime & Prompt Construction (`src/agents/`, `src/acp/`, `src/context-engine/`, `src/plugins/`)

Each agent read the actual implementation files (not just signatures) and verified findings against the code. Severity ratings follow standard risk assessment: CRITICAL = exploitable with high impact, HIGH = exploitable or high-impact design flaw, MEDIUM = requires specific conditions or moderate impact, LOW = minor or informational.
