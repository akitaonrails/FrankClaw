# FrankClaw

A security-hardened personal AI assistant gateway written in Rust. Connects messaging channels to AI model providers through a local WebSocket control plane.

FrankClaw is a ground-up Rust rewrite of [OpenClaw](https://github.com/openclaw/openclaw), achieving **functional parity** with the core feature set while providing **stronger security guarantees** through Rust's memory safety, encryption at rest, stricter input validation, and defense-in-depth hardening at every layer.

What's at parity:
- 7 messaging channels: Web, Telegram, Discord, Slack, Signal, WhatsApp, Email (IMAP/SMTP)
- Multi-provider AI with failover (OpenAI, Anthropic, Ollama)
- Full agent runtime: context compaction, subagent orchestration, command system, skills, hooks
- Media pipeline with vision/audio understanding
- Canvas host with revision conflict detection
- Browser automation (CDP-based, 9 tools)
- Bash tool with allowlist + sandbox (ai-jail)
- 3-tier tool risk levels (ReadOnly → Mutating → Destructive) with per-tool approval overrides
- Operator experience: setup wizard, doctor diagnostics, security audit, daemon management

What's intentionally skipped (low value or over-engineered):
- TTS, polls, WhatsApp Web (Baileys), Gmail Pub/Sub, ACP protocol, auto-update, i18n
- 17 long-tail channels (Google Chat, iMessage, IRC, Teams, Matrix, etc.) — can be added via the plugin trait

For full details, see [PARITY_TODO.md](PARITY_TODO.md) and [FEATURE_PLANS.md](FEATURE_PLANS.md).
For channel setup, see [CHANNEL_SETUP.md](CHANNEL_SETUP.md), `examples/channels/`, or `frankclaw config-example --channel <name>`.

## Features

- **Multi-channel messaging** — Web, Telegram, Discord, Slack, Signal, WhatsApp, Email (IMAP/SMTP)
- **Multi-provider AI** — OpenAI, Anthropic, Ollama with automatic failover
- **Encrypted sessions** — SQLite-backed with ChaCha20-Poly1305 encryption at rest
- **Scheduled jobs** — Cron-based task scheduling with agent delivery
- **Canvas host** — local authenticated visual workspace surface
- **Bounded tools** — session inspection plus Chromium-backed `browser.open`, `browser.extract`, `browser.snapshot`, `browser.click`, `browser.type`, `browser.wait`, `browser.press`, `browser.sessions`, and `browser.close`
- **3-tier tool risk levels** — Tools are classified as ReadOnly, Mutating, or Destructive. Approval gates are controlled via `FRANKCLAW_TOOL_APPROVAL` with per-tool overrides.
- **Bash tool** — Shell command execution with timeout, output truncation, and configurable security policy (deny-all, allowlist, or allow-all)
- **Optional sandbox** — [ai-jail](https://github.com/akitaonrails/ai-jail) integration (bubblewrap + landlock) for OS-level command isolation, complementary to the bash allowlist
- **Operator support** — interactive setup wizard, doctor diagnostics, security audit with severity ratings, process management (start/stop daemon), status, remote exposure checks, onboarding, and systemd unit generation
- **Docker runtime** — `docker compose up gateway chromium` starts the gateway plus a local DevTools endpoint for browser tools
- **Prompt templates** — All LLM-facing text lives in editable markdown files, embedded at compile time
- **Media pipeline** — File handling with SSRF protection, filename sanitization, and optional VirusTotal malware scanning
- **Plugin system** — Trait-based channel and provider adapters
- **Zero unsafe code** — `#![forbid(unsafe_code)]` on every crate

## Architecture

```
┌─────────────────────────────────────────────┐
│           CLI / Control UI / Apps           │
├─────────────────────────────────────────────┤
│         Gateway (WebSocket + HTTP)          │
│  ┌──────┬───────┬──────┬──────┬─────────┐  │
│  │ Auth │ Proto │ Cron │Hooks │ Sessions│  │
│  └──────┴───────┴──────┴──────┴─────────┘  │
├─────────────────────────────────────────────┤
│            Model Providers                  │
│  ┌────────┬───────────┬────────┐            │
│  │ OpenAI │ Anthropic │ Ollama │            │
│  └────────┴───────────┴────────┘            │
├─────────────────────────────────────────────┤
│           Channel Adapters                  │
│  ┌──────────┬─────┬─────────┬───────────┐   │
│  │ Telegram │ Web │ Discord │ Slack ... │   │
│  └──────────┴─────┴─────────┴───────────┘   │
├─────────────────────────────────────────────┤
│              Storage                        │
│  ┌──────────┬───────┬────────┐              │
│  │ Sessions │ Media │ Memory │              │
│  │ (SQLite) │(Files)│(Vector)│              │
│  └──────────┴───────┴────────┘              │
└─────────────────────────────────────────────┘
```

### Crate Map

| Crate | Description |
|-------|-------------|
| `frankclaw-core` | Shared types, traits, error hierarchy, SSRF IP blocklist |
| `frankclaw-crypto` | ChaCha20-Poly1305 encryption, Argon2id hashing, HMAC-SHA256 key derivation |
| `frankclaw-gateway` | Axum WebSocket + HTTP server, auth, rate limiting, config hot-reload |
| `frankclaw-sessions` | SQLite session store with optional encrypted transcripts |
| `frankclaw-models` | AI provider adapters (OpenAI, Anthropic, Ollama) with failover chain |
| `frankclaw-channels` | Messaging channel adapters (Web, Telegram, Discord, Slack, Signal, WhatsApp, Email) |
| `frankclaw-runtime` | Agent runtime, prompt templates, subagent orchestration, context compaction |
| `frankclaw-tools` | Tool registry, bash execution (with optional ai-jail sandbox), browser tools |
| `frankclaw-memory` | Vector search traits for long-term memory |
| `frankclaw-cron` | Scheduled job service |
| `frankclaw-media` | File storage with SSRF-safe HTTP fetcher and optional VirusTotal malware scanning |
| `frankclaw-plugin-sdk` | Plugin registry for extending channels and tools |
| `frankclaw-cli` | CLI binary with all subcommands |

## Requirements

- **Rust 1.93+** (edition 2024)
- **SQLite** (bundled via `rusqlite`, no system install needed)
- **Optional:** Ollama for local model inference

## Quick Start

### 1. Build

```bash
git clone https://github.com/frankclaw/frankclaw.git
cd frankclaw
cargo build --release
```

The binary is at `target/release/frankclaw`.

### 2. Initialize Configuration

```bash
./target/release/frankclaw onboard --channel web
```

This creates `~/.local/share/frankclaw/frankclaw.json` with secure defaults and `0600` file permissions.
Use `--channel telegram`, `--channel whatsapp`, `--channel slack`, `--channel discord`, `--channel signal`, or `--channel email` to start from a channel-specific profile.

### 3. Generate an Auth Token

```bash
./target/release/frankclaw gen-token
```

Copy the output token (256-bit, base64url-encoded) and add it to your config:

```json
{
  "gateway": {
    "auth": {
      "mode": "token",
      "token": "YOUR_TOKEN_HERE"
    }
  }
}
```

### 4. Configure a Model Provider

Add at least one AI provider to your config. For local-only setup with Ollama:

```json
{
  "models": {
    "providers": [
      {
        "id": "ollama",
        "api": "ollama",
        "base_url": "http://127.0.0.1:11434"
      }
    ],
    "default_model": "llama3"
  }
}
```

For OpenAI or Anthropic, set the API key via environment variable:

```bash
export OPENAI_API_KEY="sk-..."
# or
export ANTHROPIC_API_KEY="sk-ant-..."
```

And add the provider to config:

```json
{
  "models": {
    "providers": [
      {
        "id": "openai",
        "api": "openai",
        "base_url": "https://api.openai.com/v1",
        "api_key_ref": "OPENAI_API_KEY",
        "models": ["gpt-4o", "gpt-4o-mini"]
      }
    ]
  }
}
```

### 5. Start the Gateway

```bash
./target/release/frankclaw gateway
```

The gateway starts on `127.0.0.1:18789` by default. Connect via WebSocket for the control protocol.

### 6. Validate Configuration

```bash
./target/release/frankclaw check
./target/release/frankclaw doctor
./target/release/frankclaw status
```

### 7. Run Tests

```bash
cargo test
```

## Browser Tools

FrankClaw can now drive a real Chromium instance over the DevTools protocol for safe browser-backed page sessions.

### Local Dev Mode

You can run Chromium directly on the host:

```bash
chromium \
  --headless=new \
  --disable-gpu \
  --no-sandbox \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/frankclaw-chromium \
  about:blank
```

### Docker Compose Mode

```bash
cp examples/channels/web.json frankclaw.json
docker compose up -d gateway chromium
```

This starts a local Chromium container exposing DevTools on `127.0.0.1:9222`.

If you need a non-default endpoint, set:

```bash
export FRANKCLAW_BROWSER_DEVTOOLS_URL="http://127.0.0.1:9222/"
```

Then allow browser tools on an agent:

```json
{
  "agents": {
    "default_agent": "default",
    "agents": {
      "default": {
        "tools": [
          "session.inspect",
          "browser.open",
          "browser.extract",
          "browser.snapshot",
          "browser.click",
          "browser.type",
          "browser.wait",
          "browser.press",
          "browser.sessions",
          "browser.close"
        ]
      }
    }
  }
}
```

Example use:

```bash
frankclaw tools invoke --tool browser.open --session default:web:control --args '{"url":"https://example.com"}'
frankclaw tools invoke --tool browser.extract --session default:web:control
frankclaw tools invoke --tool browser.snapshot --session default:web:control
FRANKCLAW_TOOL_APPROVAL=mutating \
frankclaw tools invoke --tool browser.type --session default:web:control --args '{"selector":"input[name=q]","text":"frankclaw"}'
FRANKCLAW_TOOL_APPROVAL=mutating \
frankclaw tools invoke --tool browser.click --session default:web:control --args '{"selector":"button[type=submit]"}'
frankclaw tools invoke --tool browser.wait --session default:web:control --args '{"selector":"#results","timeout_ms":2000}'
FRANKCLAW_TOOL_APPROVAL=mutating \
frankclaw tools invoke --tool browser.press --session default:web:control --args '{"selector":"input[name=q]","key":"Enter"}'
frankclaw tools invoke --tool browser.sessions --session default:web:control
frankclaw tools invoke --tool browser.close --session default:web:control
```

`browser.click`, `browser.type`, `browser.press`, and `browser.select_option` are classified as **Mutating** tools and require `FRANKCLAW_TOOL_APPROVAL=mutating` (or the legacy `FRANKCLAW_ALLOW_BROWSER_MUTATIONS=1`).

Live regression check against a real local Chromium instance:

```bash
FRANKCLAW_BROWSER_DEVTOOLS_URL=http://127.0.0.1:9223/ \
  cargo test -p frankclaw-tools browser_tools_drive_real_chromium -- --ignored
```

## CLI Reference

```
frankclaw gateway         Start the gateway server
frankclaw gen-token       Generate a 256-bit auth token
frankclaw hash-password   Hash a password with Argon2id for config
frankclaw setup           Interactive setup wizard (provider, channel, auth)
frankclaw onboard         Create a starter config for a supported channel profile
frankclaw init            Create a blank config with secure defaults
frankclaw check           Validate config file
frankclaw doctor          Run high-signal validation and readiness checks
frankclaw audit           Security audit with severity-rated findings
frankclaw start           Start the gateway as a background daemon
frankclaw stop            Stop the running daemon
frankclaw config-example  Print a supported channel config snippet
frankclaw status          Show runtime and exposure status
frankclaw remote-status   Show remote exposure posture
frankclaw install-systemd Print a systemd unit for the current install
frankclaw config          Show resolved configuration (secrets redacted)
frankclaw tools list      Show tools allowed for an agent
frankclaw tools invoke    Invoke a configured tool locally
frankclaw tools activity  Show recent tool activity for a session
frankclaw message-delete-last  Delete the last tracked reply for a session
```

### Global Options

```
-c, --config <PATH>       Config file path (env: FRANKCLAW_CONFIG)
    --state-dir <PATH>    State directory (env: FRANKCLAW_STATE_DIR)
    --log-level <LEVEL>   Log level: trace|debug|info|warn|error (env: FRANKCLAW_LOG)
```

### Gateway Options

```
frankclaw gateway -p 9000   Override listen port
```

## Security

FrankClaw is designed with defense-in-depth. Every layer enforces its own security boundaries. A comprehensive audit of both FrankClaw and OpenClaw (see [OPENCLAW_SECURITY_AUDIT.md](OPENCLAW_SECURITY_AUDIT.md)) confirms that FrankClaw resolves every critical and high-severity vulnerability found in the reference implementation.

### Why FrankClaw is More Secure Than OpenClaw

| Area | OpenClaw | FrankClaw |
|------|----------|-----------|
| **Memory safety** | JavaScript (GC, no buffer overflows) | Rust with `#![forbid(unsafe_code)]` — no unsafe blocks anywhere |
| **Encryption at rest** | Plaintext transcripts and config on disk | ChaCha20-Poly1305 encryption with master key |
| **Password hashing** | No password auth mode found | Argon2id (t=3, m=64MB, p=4) |
| **Token comparison** | SHA-256 + timingSafeEqual, but type-check short-circuit leaks timing | Constant-time byte comparison, no early returns |
| **Shell execution** | No mandatory command allowlist; `eval()` in browser tool | Deny-all default + binary allowlist + metacharacter rejection + optional ai-jail sandbox |
| **Webhook auth** | Discord: hardcoded placeholder key; Slack: zero signature verification | Application-layer signature validation on all channels |
| **File permissions** | `0o644` (world-readable) for media files | `0o600` (owner-only) for everything |
| **Session encryption** | None — all conversation history readable on disk | ChaCha20-Poly1305 when master key is set |
| **Prompt injection** | `sanitizeForPromptLiteral()` + `<untrusted-text>` wrapping | Unicode Cc/Cf stripping on all inputs + tool outputs, external content tags, 2 MB prompt size limit, no user data in system prompts |
| **Input validation** | No identifier length limits, no WebSocket frame size enforcement | 255-byte ID limits, 800-byte session key limits, configurable WS frame size |
| **Malware scanning** | None | Optional VirusTotal integration on all file uploads |
| **Sandbox** | Docker with gaps (no `--cap-drop=ALL`, `eval()` in browser, symlink bypass) | ai-jail (bubblewrap + landlock), read-only lockdown mode available |
| **Security audit CLI** | `secrets/audit.ts` (detects plaintext secrets only) | `frankclaw audit` with 7 categories, severity ratings, CI exit codes |
| **Prototype pollution** | Explicitly guarded in config merge | N/A — Rust has no prototype chain |
| **OAuth/credential storage** | Plaintext files in `~/.openclaw/credentials/` | Encrypted via master key |
| **Session fixation** | User-controlled session key header with no validation | Session keys validated against agent ownership |

OpenClaw's audit found **7 CRITICAL** and **9 HIGH** severity issues. FrankClaw addresses all of them by design or explicit mitigation.

### What's Hardened

| Area | Implementation |
|------|---------------|
| **Memory safety** | `#![forbid(unsafe_code)]` on all crates. Rust ownership prevents buffer overflows, use-after-free, and data races. |
| **Session storage** | SQLite with WAL mode and `PRAGMA secure_delete = ON`. Transcript content encrypted with ChaCha20-Poly1305 when a master key is provided. |
| **Password hashing** | Argon2id with OWASP-recommended parameters (t=3, m=64MB, p=4). |
| **Token comparison** | Constant-time byte comparison prevents timing side-channels. |
| **Secret handling** | All secrets wrapped in `SecretString` (zeroed from memory on drop, prints `[REDACTED]` in Debug/logs). |
| **File permissions** | All sensitive files created with `0600` (owner-only). Directories `0700`. |
| **Network binding** | Gateway **refuses to start** if bound to a non-loopback address without authentication configured. This is a hard error, not a warning. |
| **SSRF protection** | All outbound HTTP requests resolve DNS first and block connections to private IPs (RFC 1918), loopback, link-local, CGNAT (100.64.0.0/10), documentation ranges, benchmarking ranges, and IPv4-mapped IPv6 private addresses. |
| **Prompt injection** | Unicode control/format chars (Cc, Cf) stripped from all user input and tool output before LLM ingestion. External content wrapped in boundary tags. Total prompt hard-capped at 2 MB. |
| **Media files** | Filenames sanitized (path traversal stripped, leading dots removed). MIME types mapped to safe extensions only (never `.exe`, `.sh`, `.bat`). Optional VirusTotal malware scanning before storage. |
| **Config hot-reload** | File watcher plus lock-free `ArcSwap` swap for the reloadable gateway subset. Restart-sensitive config changes are detected and flagged instead of being silently applied. |
| **Rate limiting** | Per-IP auth failure tracking with sliding window and lockout. Cleared on successful auth. |
| **Dependencies** | No OpenSSL (uses `rustls` only). Release builds use LTO, stripped symbols, and `panic = abort`. |

### Intentionally Open Surfaces

These components **must** remain open for the system to function. Understand the trade-offs:

#### 1. Channel Bot Tokens

Bot tokens for Telegram, Discord, Slack, etc. are sent to those platforms over HTTPS. If a token leaks, an attacker can impersonate your bot. **Mitigation:** store tokens encrypted, rotate regularly, use IP allowlists where the platform supports them.

#### 2. Gateway WebSocket Port

The gateway must accept TCP connections to function. **Mitigation:** binds to `127.0.0.1` by default. Use Tailscale or a VPN for remote access. Auth is **required** for any non-loopback bind.

#### 3. Model Provider API Keys

API keys are sent to OpenAI/Anthropic/Google in HTTP headers. **Mitigation:** keys are never logged (redaction layer), encrypted at rest, and you should set spending limits at the provider's dashboard.

#### 4. Webhook Endpoints

Some channels require public webhook URLs. **Mitigation:** always configure webhook signature verification. FrankClaw validates per-platform signatures (Telegram secret token, Slack signing secret, Discord Ed25519) where available.

#### 5. Media Files in Sandbox Mode

Files shared into sandboxed environments (ai-jail, Docker) are accessible to agent code. **Mitigation:** use a dedicated ephemeral media directory, read-only bind mounts where possible, and automatic cleanup after sandbox exits. With `ai-jail --lockdown`, the filesystem is read-only by default.

#### 6. Memory Vector Embeddings

Vector embeddings cannot be encrypted if you want semantic search to work. They partially encode the original text content. **Mitigation:** use local embedding models (Ollama) to avoid sending content to external APIs. Text content is encrypted at rest; only vectors remain searchable.

#### 7. Config and Environment Variables

The config file and `.env` may contain API keys and tokens. **Mitigation:** `0600` file permissions, encrypted config mode (master passphrase), and never commit these files to version control.

### Security Recommendations

1. **Always use auth** — Run `frankclaw gen-token` and configure token auth before exposing the gateway to any network.
2. **Use local models for privacy** — Ollama keeps all inference on-device. No data leaves your machine.
3. **Set provider spending limits** — Configure hard spending caps in your OpenAI/Anthropic dashboard.
4. **Rotate tokens regularly** — Bot tokens and API keys should be rotated on a schedule.
5. **Monitor logs** — Run with `--log-level info` minimum. Auth failures and SSRF blocks are logged.
6. **Keep Rust updated** — Run `rustup update` to get security fixes in the compiler and standard library.
7. **Audit dependencies** — Run `cargo audit` before deploying. Add `cargo-deny` to CI.

## FrankClaw vs IronClaw

[IronClaw](https://github.com/nearai/ironclaw) (NEAR AI) is another Rust rewrite of OpenClaw. The two projects share the same ancestor but make fundamentally different design choices. They are complementary, not competing.

| Dimension | IronClaw | FrankClaw |
|-----------|----------|-----------|
| **Deployment** | Requires PostgreSQL 15+ with pgvector | Single binary, embedded SQLite — zero external dependencies |
| **Sandbox model** | WASM (wasmtime) — tools run inside a WebAssembly VM | OS-level (bubblewrap + landlock via ai-jail) — processes run in a Linux namespace jail |
| **Channel adapters** | WASM-based plugin channels | 7 native compiled-in adapters (Web, Telegram, Discord, Slack, Signal, WhatsApp, Email) |
| **Tool approval** | Capability-based permissions per workspace | 3-tier risk levels (ReadOnly/Mutating/Destructive) with per-tool overrides |
| **Encryption at rest** | AES-256-GCM credential vault | ChaCha20-Poly1305 for sessions, config, and credentials |
| **Memory / search** | PostgreSQL pgvector + FTS with reciprocal rank fusion | Vector search trait (LanceDB backend planned), SQLite FTS |
| **Default AI provider** | NEAR AI (with OpenRouter, Together, Fireworks, Ollama) | Any OpenAI-compatible API, Anthropic, Ollama |
| **Streaming** | SSE + WebSocket web gateway | WebSocket control protocol |
| **Routines** | Built-in cron + event-driven + webhook routines engine | Cron scheduler with agent delivery |
| **Operator CLI** | Basic CLI | Full CLI: setup wizard, doctor, audit (severity-rated), daemon, systemd, onboarding |
| **Prompt injection defense** | Not documented | Unicode Cc/Cf stripping, external content boundary tags, 2 MB prompt limit |
| **Malware scanning** | Not documented | Optional VirusTotal integration on file uploads |

### When to choose which

**Choose IronClaw** if you want WASM-based tool sandboxing, need PostgreSQL-backed vector search today, or are building on the NEAR AI ecosystem.

**Choose FrankClaw** if you want a single-binary deployment with no database server, need native channel adapters that work out of the box, want OS-level sandboxing via bubblewrap/landlock, or need defense-in-depth security hardening (encryption at rest, prompt injection defense, audit CLI, malware scanning).

## Configuration Reference

FrankClaw uses a single JSON config file. All fields have secure defaults.

```jsonc
{
  // Gateway server settings
  "gateway": {
    "port": 18789,              // TCP port
    "bind": "loopback",         // "loopback", "lan", or a specific IP
    "auth": {
      "mode": "token",          // "none", "token", "password", "trusted_proxy", "tailscale"
      "token": "..."            // 256-bit base64url token (from gen-token)
    },
    "rate_limit": {
      "max_attempts": 5,        // Failed auths before lockout
      "window_secs": 60,        // Sliding window
      "lockout_secs": 300       // Lockout duration
    },
    "max_ws_message_bytes": 4194304,  // 4 MB
    "max_connections": 64
  },

  // Agent definitions
  "agents": {
    "default_agent": "default",
    "agents": {
      "default": {
        "name": "Default Agent",
        "model": "gpt-4o",
        "system_prompt": "You are a helpful assistant.",
        "sandbox": { "mode": "none" }
      }
    }
  },

  // Model providers (tried in order for failover)
  "models": {
    "providers": [
      {
        "id": "openai",
        "api": "openai",
        "base_url": "https://api.openai.com/v1",
        "api_key_ref": "OPENAI_API_KEY",
        "models": ["gpt-4o", "gpt-4o-mini"],
        "cooldown_secs": 60
      }
    ],
    "default_model": "gpt-4o"
  },

  // Session management
  "session": {
    "scoping": "main",         // "main", "per_peer", "per_channel_peer", "global"
    "reset": {
      "daily_at_hour": null,   // UTC hour (0-23) or null
      "idle_timeout_secs": null,
      "max_entries": 500
    },
    "pruning": {
      "max_age_days": 30,
      "max_sessions_per_agent": 500,
      "disk_budget_bytes": 10485760  // 10 MB
    }
  },

  // Security settings
  "security": {
    "encrypt_sessions": true,   // ChaCha20-Poly1305 encryption at rest
    "encrypt_media": false,     // Optional media encryption (performance trade-off)
    "ssrf_protection": true,    // Block fetches to private IP ranges
    "max_webhook_body_bytes": 1048576  // 1 MB
  },

  // Media pipeline
  "media": {
    "max_file_size_bytes": 5242880,  // 5 MB
    "ttl_hours": 2
  },

  // Logging
  "logging": {
    "level": "info",           // trace, debug, info, warn, error
    "format": "pretty",       // "pretty", "json", "compact"
    "redact_secrets": true     // Replace secrets with [REDACTED] in logs
  }
}
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `FRANKCLAW_CONFIG` | Path to config file |
| `FRANKCLAW_STATE_DIR` | State directory (sessions, media, logs) |
| `FRANKCLAW_LOG` | Log level override |
| `OPENAI_API_KEY` | OpenAI API key |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token |
| `FRANKCLAW_BASH_POLICY` | Bash tool policy: `deny-all` (default), `allow-all`, or comma-separated binary allowlist |
| `FRANKCLAW_SANDBOX` | Optional sandbox: `ai-jail` or `ai-jail-lockdown` (requires [ai-jail](https://github.com/akitaonrails/ai-jail)) |
| `FRANKCLAW_TOOL_APPROVAL` | Tool approval level: `readonly` (default), `mutating`, or `destructive` |
| `FRANKCLAW_ALLOW_BROWSER_MUTATIONS` | Legacy — set to `1` to enable mutating tools (use `FRANKCLAW_TOOL_APPROVAL` instead) |
| `FRANKCLAW_BROWSER_DEVTOOLS_URL` | Chromium DevTools endpoint (default: `http://127.0.0.1:9222/`) |
| `VIRUSTOTAL_API_KEY` | Optional VirusTotal API key — enables malware scanning on all file uploads |

## Development

### Running in Dev Mode

```bash
# Watch for changes and rebuild
cargo watch -x 'run -- gateway'

# Run with debug logging
FRANKCLAW_LOG=debug cargo run -- gateway

# Run specific tests
cargo test -p frankclaw-crypto
cargo test -p frankclaw-sessions
cargo test -p frankclaw-media
```

### Project Structure

```
frankclaw/
├── Cargo.toml                 # Workspace root
├── CLAUDE.md                  # AI assistant development guide
├── OPENCLAW_ANALYSIS.md       # Original OpenClaw analysis & rewrite plan
├── crates/
│   ├── frankclaw-core/        # Shared types and traits
│   ├── frankclaw-crypto/      # Cryptographic primitives
│   ├── frankclaw-gateway/     # WebSocket + HTTP server
│   ├── frankclaw-sessions/    # SQLite session store
│   ├── frankclaw-models/      # AI model providers
│   ├── frankclaw-channels/    # Messaging channel adapters
│   ├── frankclaw-memory/      # Vector memory traits
│   ├── frankclaw-cron/        # Scheduled jobs
│   ├── frankclaw-runtime/     # Agent runtime & prompt templates
│   ├── frankclaw-tools/       # Tool registry, bash & browser tools
│   ├── frankclaw-media/       # Media file handling
│   ├── frankclaw-plugin-sdk/  # Plugin system
│   └── frankclaw-cli/         # CLI binary
└── target/                    # Build artifacts (gitignored)
```

### Adding New Functionality

**New channel adapter:**
1. Create `crates/frankclaw-channels/src/<name>.rs`
2. Implement `ChannelPlugin` trait from `frankclaw-core`
3. Export from `crates/frankclaw-channels/src/lib.rs`

**New model provider:**
1. Create `crates/frankclaw-models/src/<name>.rs`
2. Implement `ModelProvider` trait from `frankclaw-core`
3. Export from `crates/frankclaw-models/src/lib.rs`

## Roadmap

See [PARITY_TODO.md](PARITY_TODO.md) for the current parity tracker.

- [ ] Long-tail attachment/media edge cases on supported channels
- [x] Streaming SSE response handling for OpenAI/Anthropic model providers
- [x] Agent runtime with optional ai-jail sandbox (bubblewrap + landlock)
- [ ] LanceDB vector memory backend
- [ ] Companion nodes and apps
- [ ] Voice

## License

MIT
