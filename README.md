# FrankClaw

A security-hardened personal AI assistant gateway written in Rust. Connects messaging channels to AI model providers through a local WebSocket control plane.

FrankClaw is a ground-up Rust rewrite of [OpenClaw](https://github.com/openclaw/openclaw), preserving the architectural vision while replacing the TypeScript implementation with memory-safe abstractions and stricter security defaults.

Current scope and parity status:
- supported channels: `web`, `telegram`, `discord`, `slack`, `signal`, `whatsapp`
- local Canvas host
- bounded tool orchestration with Chromium-backed browser sessions
- operator onboarding and install helpers

For the remaining distance to OpenClaw feature parity, see [PARITY_TODO.md](PARITY_TODO.md) and [FEATURE_PLANS.md](FEATURE_PLANS.md).
For concrete setup snippets for the supported channels and browser runtime, see [CHANNEL_SETUP.md](CHANNEL_SETUP.md).

## Features

- **Multi-channel messaging** — Web, Telegram, Discord, Slack, Signal, WhatsApp
- **Multi-provider AI** — OpenAI, Anthropic, Ollama with automatic failover
- **Encrypted sessions** — SQLite-backed with ChaCha20-Poly1305 encryption at rest
- **Scheduled jobs** — Cron-based task scheduling with agent delivery
- **Canvas host** — local authenticated visual workspace surface
- **Bounded tools** — session inspection plus Chromium-backed `browser.open`, `browser.extract`, `browser.snapshot`, `browser.click`, and `browser.type`
- **Operator support** — doctor, status, remote exposure checks, onboarding, and systemd unit generation
- **Docker browser runtime** — `docker compose up chromium` starts a local DevTools endpoint for browser tools
- **Media pipeline** — File handling with SSRF protection and filename sanitization
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
| `frankclaw-channels` | Messaging channel adapters (Web, Telegram, Discord, Slack, Signal, WhatsApp) |
| `frankclaw-memory` | Vector search traits for long-term memory |
| `frankclaw-cron` | Scheduled job service |
| `frankclaw-media` | File storage with SSRF-safe HTTP fetcher |
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
Use `--channel telegram`, `--channel whatsapp`, `--channel slack`, `--channel discord`, or `--channel signal` to start from a channel-specific profile.

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
docker compose up -d chromium
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
          "browser.snapshot"
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
frankclaw tools invoke --tool browser.type --session default:web:control --args '{"selector":"input[name=q]","text":"frankclaw"}'
frankclaw tools invoke --tool browser.click --session default:web:control --args '{"selector":"button[type=submit]"}'
```

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
frankclaw onboard         Create a starter config for a supported channel profile
frankclaw init            Create a blank config with secure defaults
frankclaw check           Validate config file
frankclaw doctor          Run high-signal validation and readiness checks
frankclaw status          Show runtime and exposure status
frankclaw remote-status   Show remote exposure posture
frankclaw install-systemd Print a systemd unit for the current install
frankclaw config          Show resolved configuration (secrets redacted)
frankclaw tools list      Show tools allowed for an agent
frankclaw tools invoke    Invoke a configured tool locally
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

FrankClaw is designed with defense-in-depth. Every layer enforces its own security boundaries.

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
| **Media files** | Filenames sanitized (path traversal stripped, leading dots removed). MIME types mapped to safe extensions only (never `.exe`, `.sh`, `.bat`). |
| **Config hot-reload** | Lock-free `ArcSwap` pointer swap. No race conditions between in-flight requests and config updates. |
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

Files shared into Docker/Podman sandboxes are accessible to agent code. **Mitigation:** use a dedicated ephemeral media directory, read-only bind mounts where possible, and automatic cleanup after sandbox exits.

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

- [ ] Streaming SSE response handling for model providers
- [ ] Discord, Slack, Signal channel adapters
- [ ] Agent runtime with sandbox (Bubblewrap/Docker/Podman)
- [ ] LanceDB vector memory backend
- [ ] Config file watcher for hot-reload
- [ ] Device node pairing (mobile/desktop apps)
- [ ] Minimal control UI (Tailwind)

## License

MIT
