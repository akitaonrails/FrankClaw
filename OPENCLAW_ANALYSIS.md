# OpenClaw Analysis & Rust Rewrite Plan ("FrankClaw")

## Part 1: OpenClaw Architecture Analysis

### Overview

OpenClaw is a personal AI assistant gateway (~912K lines of TypeScript) that:
- Connects to 20+ messaging channels (Telegram, Discord, WhatsApp, Slack, Signal, iMessage, etc.)
- Routes messages to AI model providers (OpenAI, Anthropic, Google, Ollama, etc.)
- Manages sessions, memory, tools, scheduled jobs, and multi-agent orchestration
- Runs as a local daemon with WebSocket control plane

### Core Architecture (5 Layers)

```
┌─────────────────────────────────────────────────────┐
│  Control UI (Web)  │  CLI  │  Mobile Apps (iOS/And) │  ← Clients
├─────────────────────────────────────────────────────┤
│              Gateway (WS + HTTP Server)              │  ← Orchestration
│   ┌──────┬──────┬──────┬──────┬──────┬──────────┐   │
│   │ Auth │ Proto│Routes│ Cron │Hooks │ Sessions  │   │
│   └──────┴──────┴──────┴──────┴──────┴──────────┘   │
├─────────────────────────────────────────────────────┤
│              Agent Runtime (ACP)                     │  ← Execution
│   ┌──────┬──────┬──────┬──────┬──────┐              │
│   │Skills│Tools │Memory│Models│Sandbox│              │
│   └──────┴──────┴──────┴──────┴──────┘              │
├─────────────────────────────────────────────────────┤
│              Channel Adapters                        │  ← I/O
│   ┌────┬───────┬────┬─────┬──────┬─────┬────────┐   │
│   │ TG │Discord│ WA │Slack│Signal│ IRC │ 15 more│   │
│   └────┴───────┴────┴─────┴──────┴─────┴────────┘   │
├─────────────────────────────────────────────────────┤
│              Storage                                 │  ← Persistence
│   ┌────────┬──────────┬───────┬──────────────────┐   │
│   │Sessions│  Config  │ Media │ Memory (LanceDB) │   │
│   │ (JSONL)│(JSON5/Y) │(Files)│  (Vector+SQLite) │   │
│   └────────┴──────────┴───────┴──────────────────┘   │
└─────────────────────────────────────────────────────┘
```

### Feature Inventory

| Feature | Description | Complexity |
|---------|-------------|------------|
| **Gateway Server** | WS + HTTP hybrid, config hot-reload, TLS | High |
| **Protocol** | AJV-validated JSON-RPC over WebSocket | Medium |
| **Authentication** | Token, password, Tailscale, trusted-proxy | Medium |
| **Session Management** | Per-sender/peer/channel/global/thread scoping, JSONL storage, pruning, disk budgets | High |
| **Agent Runtime (ACP)** | Process isolation, turn tracking, error recovery, sandbox (Docker/Podman) | Very High |
| **Model Providers** | OpenAI, Anthropic, Google, Ollama, Bedrock, Copilot APIs with failover | High |
| **Channel Adapters** | 20+ channels with unified plugin interface | Very High (breadth) |
| **Tools** | Browser automation, canvas, cron, webhooks, agent-step | High |
| **Skills** | Bundled + managed + workspace skills, frontmatter parsing | Medium |
| **Memory** | LanceDB vector DB, embedding providers, hybrid search, temporal decay | High |
| **Cron** | Scheduled jobs, heartbeat delivery, run logs, retry | Medium |
| **Hooks** | Pre/post request hooks with agent policy | Low-Medium |
| **Media Pipeline** | Upload/download, SSRF protection, MIME detection, TTL cleanup | Medium |
| **Config System** | JSON5/YAML, Zod validation, schema generation, env substitution, hot-reload | High |
| **Plugin SDK** | Channel adapters, tool factories, config schemas with UI hints | High |
| **Streaming** | Block streaming, draft chunking, coalescing for long responses | Medium |
| **Device Nodes** | iOS/Android/macOS native apps, Bonjour discovery, device pairing | High |

### Security Model (Current)

**Strengths:**
- Pairing policy for DMs (unknown senders get approval code)
- Allowlist/blocklist per channel
- SSRF protection on media fetches (pinned hostname resolution)
- Approval workflows for system commands
- Content Security Policy on Control UI
- Optional Docker/Podman sandboxing
- Rate limiting on auth and control-plane writes

**Weaknesses / Areas for Improvement:**
- File-based session storage (no encryption at rest)
- Credentials stored as plaintext JSON files
- Media files world-readable (0o644) for Docker compat
- No mTLS between gateway and channel processes
- WebSocket protocol relies on single Bearer token
- Config hot-reload could race with in-flight requests
- JSONL session files can grow unbounded before pruning kicks in

---

## Part 2: Rust Rewrite Plan — "FrankClaw"

### Design Philosophy

1. **Memory safety by default** — Rust's ownership model eliminates entire classes of vulnerabilities
2. **Zero-cost abstractions** — Trait-based plugin system with no runtime reflection
3. **Explicit error handling** — `Result<T, E>` everywhere, no silent failures
4. **Defense in depth** — Encrypt at rest, validate all boundaries, minimize trust
5. **Minimal dependencies** — Audit every crate, prefer well-maintained ecosystem crates

### Crate Ecosystem Selection

| Component | Crate(s) | Rationale |
|-----------|----------|-----------|
| Async runtime | `tokio` | Industry standard, mature |
| HTTP/WS server | `axum` + `tokio-tungstenite` | Type-safe, tower middleware |
| Serialization | `serde` + `serde_json` | De facto standard |
| Config | `config` + `serde_yaml` | Multi-format, env overlay |
| Validation | Custom derive macros + `validator` | Compile-time where possible |
| Crypto | `ring` or `rustls` + `argon2` | Audited, no OpenSSL |
| Database | `rusqlite` (sessions) + `lancedb-rs` (vectors) | Embedded, no external DB |
| CLI | `clap` v4 | Derive-based, full featured |
| Logging | `tracing` + `tracing-subscriber` | Structured, async-aware |
| Channel SDKs | Per-channel crates (see below) | |
| HTTP client | `reqwest` | Mature, TLS built-in |
| Template/schema | `schemars` + `jsonschema` | JSON Schema generation |

### Project Structure

```
frankclaw/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── frankclaw-core/           # Shared types, traits, error types
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── config/           # Config types, validation, schema
│   │   │   ├── session/          # Session types, scoping, storage trait
│   │   │   ├── protocol/         # WS protocol frames, method registry
│   │   │   ├── channel/          # Channel trait + adapter types
│   │   │   ├── agent/            # Agent trait, ACP types
│   │   │   ├── model/            # Model provider trait, API types
│   │   │   ├── media/            # Media types, MIME, storage trait
│   │   │   ├── auth/             # Auth types, token, password, tailscale
│   │   │   ├── crypto/           # Encryption, hashing, key derivation
│   │   │   └── error.rs          # Unified error hierarchy
│   │
│   ├── frankclaw-gateway/        # Gateway server implementation
│   │   ├── src/
│   │   │   ├── server.rs         # Axum + WS server
│   │   │   ├── ws/               # WebSocket connection lifecycle
│   │   │   ├── http/             # HTTP routes (webhooks, probes, UI)
│   │   │   ├── auth/             # Auth middleware + rate limiting
│   │   │   ├── methods/          # RPC method handlers
│   │   │   ├── broadcast.rs      # Pub/sub to connected clients
│   │   │   ├── config_reload.rs  # Hot-reload with RwLock
│   │   │   └── state.rs          # Shared gateway state (Arc<GatewayState>)
│   │
│   ├── frankclaw-sessions/       # Session management
│   │   ├── src/
│   │   │   ├── store.rs          # SQLite-backed session store
│   │   │   ├── transcript.rs     # Transcript append/query
│   │   │   ├── pruning.rs        # TTL, disk budget enforcement
│   │   │   ├── scoping.rs        # Session key resolution
│   │   │   └── encryption.rs     # At-rest encryption for transcripts
│   │
│   ├── frankclaw-agents/         # Agent runtime
│   │   ├── src/
│   │   │   ├── runtime.rs        # ACP session manager
│   │   │   ├── spawn.rs          # Process isolation + sandbox
│   │   │   ├── turn.rs           # Turn execution + abort
│   │   │   ├── tools/            # Built-in tools (browser, cron, etc.)
│   │   │   ├── skills/           # Skill loader + frontmatter parser
│   │   │   └── sandbox.rs        # Container sandbox (Docker/Podman)
│   │
│   ├── frankclaw-models/         # Model provider adapters
│   │   ├── src/
│   │   │   ├── openai.rs         # OpenAI completions + responses
│   │   │   ├── anthropic.rs      # Anthropic messages API
│   │   │   ├── google.rs         # Google Generative AI
│   │   │   ├── ollama.rs         # Local Ollama
│   │   │   ├── bedrock.rs        # AWS Bedrock
│   │   │   ├── failover.rs       # Provider failover + rotation
│   │   │   └── streaming.rs      # SSE/streaming response handling
│   │
│   ├── frankclaw-channels/       # Channel adapter implementations
│   │   ├── src/
│   │   │   ├── telegram.rs       # Telegram Bot API
│   │   │   ├── discord.rs        # Discord gateway + REST
│   │   │   ├── slack.rs          # Slack Events API + Web API
│   │   │   ├── signal.rs         # signal-cli subprocess
│   │   │   ├── whatsapp.rs       # WhatsApp (protocol crate or bridge)
│   │   │   ├── irc.rs            # IRC protocol
│   │   │   ├── matrix.rs         # Matrix client-server API
│   │   │   ├── web.rs            # HTTP/WebSocket chat
│   │   │   └── ...               # Other channels
│   │
│   ├── frankclaw-memory/         # Memory / vector search
│   │   ├── src/
│   │   │   ├── store.rs          # LanceDB vector storage
│   │   │   ├── embeddings.rs     # Embedding provider trait + impls
│   │   │   ├── search.rs         # Hybrid search, MMR, temporal decay
│   │   │   └── capture.rs        # Auto-capture pipeline
│   │
│   ├── frankclaw-cron/           # Scheduled job system
│   │   ├── src/
│   │   │   ├── service.rs        # CronService
│   │   │   ├── schedule.rs       # Cron expression parser
│   │   │   ├── delivery.rs       # Job delivery to agents
│   │   │   ├── store.rs          # Job persistence
│   │   │   └── run_log.rs        # Execution history
│   │
│   ├── frankclaw-media/          # Media pipeline
│   │   ├── src/
│   │   │   ├── store.rs          # File storage + TTL cleanup
│   │   │   ├── fetch.rs          # Network fetch + SSRF protection
│   │   │   ├── mime.rs           # MIME detection + safe extension mapping
│   │   │   └── encryption.rs     # Optional at-rest encryption
│   │
│   ├── frankclaw-plugin-sdk/     # Plugin development SDK
│   │   ├── src/
│   │   │   ├── channel.rs        # Channel plugin trait
│   │   │   ├── tool.rs           # Tool plugin trait
│   │   │   ├── memory.rs         # Memory plugin trait
│   │   │   ├── config.rs         # Config schema helpers
│   │   │   └── registry.rs       # Plugin registration
│   │
│   └── frankclaw-cli/            # CLI binary
│       ├── src/
│       │   ├── main.rs           # Entry point
│       │   ├── commands/         # CLI subcommands
│       │   └── onboard.rs        # Interactive setup wizard
│
├── plugins/                      # Out-of-tree plugins (dynamic .so/.dylib)
│   ├── channel-msteams/
│   ├── channel-googlechat/
│   └── ...
│
└── tests/
    ├── integration/
    └── e2e/
```

---

## Part 3: Core Component Specifications

### 3.1 Gateway Server (`frankclaw-gateway`)

```rust
// Shared, thread-safe gateway state
pub struct GatewayState {
    config: ArcSwap<FrankClawConfig>,       // Lock-free config hot-reload
    sessions: Arc<SessionStore>,             // SQLite-backed
    clients: DashMap<ConnId, ClientHandle>,   // Connected WS clients
    agents: Arc<AgentManager>,               // ACP runtime
    channels: Arc<ChannelRegistry>,          // Registered channel plugins
    cron: Arc<CronService>,                  // Scheduled jobs
    media: Arc<MediaStore>,                  // Media file management
    auth: Arc<AuthProvider>,                 // Auth validation
    shutdown: CancellationToken,             // Graceful shutdown
}
```

**Key design decisions:**
- `ArcSwap<Config>` for lock-free config hot-reload (no RwLock contention)
- `DashMap` for concurrent client map (sharded locking)
- `CancellationToken` for graceful shutdown propagation
- All channel I/O via `tokio::spawn` tasks with structured concurrency

**HTTP routes (axum):**
```rust
Router::new()
    .route("/health", get(health_probe))
    .route("/ready", get(readiness_probe))
    .route("/ws", get(ws_upgrade_handler))
    .route("/webhooks/:channel/:account", post(webhook_handler))
    .nest("/ui", control_ui_routes())
    .layer(CorsLayer::new().allow_origin(/* config */))
    .layer(CompressionLayer::new())
    .layer(TimeoutLayer::new(Duration::from_secs(30)))
    .with_state(state)
```

### 3.2 Protocol (`frankclaw-core::protocol`)

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Frame {
    Request(RequestFrame),
    Response(ResponseFrame),
    Event(EventFrame),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestFrame {
    pub id: RequestId,
    pub method: Method,
    pub params: serde_json::Value,  // Validated per-method
}

// Method dispatch via enum (exhaustive matching, no string lookup)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    AgentIdentity,
    ChatSend,
    ChatHistory,
    ChannelsStatus,
    ConfigGet,
    ConfigPatch,
    ConfigApply,
    CronAdd,
    CronRemove,
    CronRun,
    SessionsPatch,
    SessionsReset,
    ModelsListAvailable,
    WebhooksAdd,
    WebhooksTest,
    LogsTail,
    // ... exhaustive list
}
```

**Validation:** Per-method parameter validation using `serde` + custom validators. Compile-time guarantees where possible via typed param structs.

### 3.3 Authentication (`frankclaw-core::auth`)

```rust
pub enum AuthMode {
    None,
    Token(SecretString),            // Constant-time comparison
    Password(Argon2Hash),           // Argon2id (not bcrypt)
    TrustedProxy { header: String },
    Tailscale,
}

pub struct AuthProvider {
    mode: AuthMode,
    rate_limiter: Arc<RateLimiter>, // Token bucket per IP
    session_tokens: DashMap<SessionToken, AuthSession>,
}
```

**Hardening vs OpenClaw:**
- `SecretString` (from `secrecy` crate) — zeroed on drop, no accidental logging
- `Argon2id` instead of bcrypt — memory-hard, side-channel resistant
- Constant-time token comparison via `ring::constant_time::verify_slices_are_equal`
- Rate limiter with exponential backoff + jitter

### 3.4 Session Store (`frankclaw-sessions`)

```rust
// SQLite-backed instead of JSONL files
pub struct SessionStore {
    db: Pool<Sqlite>,                       // r2d2 or deadpool connection pool
    encryption_key: Option<EncryptionKey>,   // ChaCha20-Poly1305
    pruning_config: PruningConfig,
}

pub struct SessionEntry {
    pub key: SessionKey,
    pub channel: ChannelId,
    pub agent_id: AgentId,
    pub scoping: SessionScoping,
    pub created_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub metadata: SessionMetadata,
}

// Transcript entries stored in separate table, encrypted at rest
pub struct TranscriptEntry {
    pub session_key: SessionKey,
    pub seq: u64,
    pub role: Role,
    pub content: EncryptedBlob,  // Encrypted with session-derived key
    pub timestamp: DateTime<Utc>,
}
```

**Why SQLite over JSONL:**
- ACID transactions (no corrupted sessions on crash)
- Efficient pruning via `DELETE WHERE`
- Built-in WAL mode for concurrent reads
- Disk budget enforcement via `PRAGMA page_count`
- Indexed lookups instead of full-file scans

### 3.5 Channel Plugin Trait (`frankclaw-plugin-sdk`)

```rust
#[async_trait]
pub trait ChannelPlugin: Send + Sync + 'static {
    fn id(&self) -> ChannelId;
    fn capabilities(&self) -> ChannelCapabilities;

    // Lifecycle
    async fn start(&self, ctx: &PluginContext) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn health(&self) -> HealthStatus;

    // Messaging
    async fn send(&self, msg: OutboundMessage) -> Result<SendResult>;
    async fn edit(&self, msg: EditMessage) -> Result<()> {
        Err(Error::Unsupported("edit"))
    }
    async fn delete(&self, msg: DeleteMessage) -> Result<()> {
        Err(Error::Unsupported("delete"))
    }

    // Streaming (optional)
    fn supports_streaming(&self) -> bool { false }
    async fn stream_start(&self, _target: &SendTarget) -> Result<StreamHandle> {
        Err(Error::Unsupported("streaming"))
    }

    // Config schema for UI generation
    fn config_schema(&self) -> schemars::schema::RootSchema;
}

// Inbound messages delivered via channel:
pub type InboundSender = mpsc::Sender<InboundMessage>;
```

**Plugin loading:** Channels can be compiled-in (static dispatch, zero overhead) or loaded as dynamic libraries (`.so`/`.dylib`) via `libloading` for third-party extensions.

### 3.6 Model Provider Trait (`frankclaw-models`)

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(
        &self,
        request: CompletionRequest,
        stream_tx: Option<mpsc::Sender<StreamDelta>>,
    ) -> Result<CompletionResponse>;

    fn capabilities(&self) -> ModelCapabilities;
    fn cost(&self) -> ModelCost;
}

pub struct FailoverChain {
    providers: Vec<Box<dyn ModelProvider>>,
    cooldowns: DashMap<ProviderId, Instant>,
}

impl FailoverChain {
    pub async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        for provider in &self.providers {
            if self.is_cooled_down(provider.id()) {
                continue;
            }
            match provider.complete(req.clone(), None).await {
                Ok(resp) => return Ok(resp),
                Err(e) if e.is_retryable() => {
                    self.set_cooldown(provider.id());
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::AllProvidersFailed)
    }
}
```

### 3.7 Agent Runtime (`frankclaw-agents`)

```rust
pub struct AgentManager {
    sessions: DashMap<SessionKey, AgentSession>,
    turn_semaphore: Semaphore,  // Limit concurrent turns
    sandbox_config: SandboxConfig,
}

pub struct AgentSession {
    pub key: SessionKey,
    pub agent_id: AgentId,
    pub runtime: AgentRuntime,
    pub active_turn: Option<TurnHandle>,
    pub created_at: Instant,
}

pub struct TurnHandle {
    pub seq: u64,
    pub cancel: CancellationToken,
    pub started_at: Instant,
}

pub enum SandboxMode {
    None,
    Docker { image: String, memory_limit: ByteSize, timeout: Duration },
    Podman { image: String, memory_limit: ByteSize, timeout: Duration },
    Bubblewrap { profile: BwrapProfile },  // Linux-native sandboxing
}
```

### 3.8 Cron Service (`frankclaw-cron`)

```rust
pub struct CronService {
    store: Arc<CronStore>,       // SQLite-backed job store
    scheduler: JoinHandle<()>,   // Background tick loop
    cancel: CancellationToken,
}

pub struct CronJob {
    pub id: CronJobId,
    pub schedule: CronSchedule,
    pub agent_id: AgentId,
    pub session_key: SessionKey,
    pub prompt: String,
    pub enabled: bool,
    pub last_run: Option<RunLog>,
    pub created_at: DateTime<Utc>,
}
```

### 3.9 Media Pipeline (`frankclaw-media`)

```rust
pub struct MediaStore {
    base_dir: PathBuf,
    max_file_size: ByteSize,
    ttl: Duration,
    encryption_key: Option<EncryptionKey>,
}

pub struct MediaFile {
    pub id: MediaId,           // UUID
    pub original_name: String,
    pub mime: Mime,
    pub size: u64,
    pub path: PathBuf,
    pub expires_at: Instant,
}

impl MediaStore {
    /// Fetch from URL with SSRF protection
    pub async fn fetch_url(&self, url: &Url) -> Result<MediaFile> {
        // 1. DNS resolution with SSRF checks (block private IPs)
        let resolved = self.resolve_safe(url).await?;
        // 2. Stream to temp file with size limit
        // 3. MIME detection + safe extension
        // 4. Move to final location
        // 5. Set permissions (0o600, NOT 0o644)
    }
}
```

---

## Part 4: Security Hardening

### 4.1 Hardened by Default (vs OpenClaw)

| Area | OpenClaw | FrankClaw |
|------|----------|-----------|
| **Session storage** | JSONL files, plaintext | SQLite + ChaCha20-Poly1305 encryption at rest |
| **Credentials** | JSON files, plaintext | OS keyring (`keyring` crate) or encrypted file with master key |
| **Media permissions** | `0o644` (world-readable) | `0o600` (owner-only); sandbox uses bind mounts |
| **Password hashing** | bcrypt | Argon2id (memory-hard) |
| **Token comparison** | String equality | Constant-time (`ring::constant_time`) |
| **Config reload** | Can race with requests | `ArcSwap` atomic pointer swap |
| **Memory** | JS garbage collector | Rust ownership; `SecretString` for secrets (zeroed on drop) |
| **Buffer overflow** | V8 protects, but native addons don't | Impossible in safe Rust |
| **Dependency audit** | npm audit | `cargo-audit` + `cargo-deny` in CI |
| **SSRF protection** | Hostname pinning | DNS rebinding protection + private IP blocklist + connect-time validation |
| **WebSocket auth** | Single bearer token | Token + per-connection nonce + optional mTLS |
| **Input validation** | AJV (runtime) | `serde` deserialization + `validator` (compile-time where possible) |
| **Subprocess isolation** | Optional Docker | Bubblewrap (Linux-native) as default + Docker/Podman |

### 4.2 Encryption Architecture

```
Master Key (derived from user passphrase via Argon2id)
    │
    ├── Config Encryption Key (HKDF-SHA256, context: "config")
    │   └── Encrypts: credentials, API keys, tokens in config
    │
    ├── Session Encryption Key (HKDF-SHA256, context: "session")
    │   └── Encrypts: transcript entries in SQLite
    │
    ├── Media Encryption Key (HKDF-SHA256, context: "media")
    │   └── Encrypts: cached media files (optional, performance trade-off)
    │
    └── Memory Encryption Key (HKDF-SHA256, context: "memory")
        └── Encrypts: vector DB text content (vectors remain unencrypted for search)
```

**Algorithm choices:**
- KDF: Argon2id (t=3, m=64MB, p=4)
- Symmetric: ChaCha20-Poly1305 (AEAD)
- Key derivation: HKDF-SHA256 with unique contexts
- All via `ring` crate (formally verified primitives)

### 4.3 Network Security

```rust
/// SSRF protection for media fetches and webhook callbacks
pub fn is_safe_target(addr: &IpAddr) -> bool {
    // Block all private/reserved ranges
    !addr.is_loopback()
        && !addr.is_private()          // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
        && !addr.is_link_local()       // 169.254.0.0/16
        && !addr.is_multicast()
        && !addr.is_unspecified()
        && !is_cgnat(addr)             // 100.64.0.0/10
        && !is_documentation(addr)     // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
        && !is_benchmarking(addr)      // 198.18.0.0/15
}
```

**TLS hardening:**
- `rustls` only (no OpenSSL)
- TLS 1.3 minimum
- Certificate pinning for known model provider APIs (optional)
- Automatic cert management via `rustls-acme` for public deployments

---

## Part 5: Security Documentation — Where Users Must Be Careful

### ⚠️ SECURITY NOTICE: Intentionally Open Surfaces

The following components **must** remain open or have reduced security for the system to function. Users must understand these trade-offs:

---

#### 1. Channel Bot Tokens (MUST be exposed to platform APIs)

**What:** Telegram bot tokens, Discord bot tokens, Slack app tokens, etc. are sent to third-party APIs over HTTPS.

**Risk:** If a token leaks, an attacker can impersonate your bot, read messages, and send as you.

**Mitigations available:**
- Store tokens in OS keyring (encrypted)
- Rotate tokens regularly via each platform's dashboard
- Use IP allowlists where platforms support them (Slack)
- Monitor bot activity logs

**What FrankClaw cannot do:** These tokens are inherently trust-bearing credentials that the platform requires. There is no way to use Telegram's Bot API without sending the token to Telegram's servers.

---

#### 2. WebSocket Gateway Port (MUST accept connections)

**What:** The gateway listens on a TCP port (default 18789) for WebSocket connections from the Control UI, mobile apps, and CLI.

**Risk:** If exposed to the internet without auth, anyone can control your assistant.

**Mitigations available:**
- Bind to loopback only (`127.0.0.1`) by default
- Use Tailscale for remote access (WireGuard-encrypted, identity-verified)
- Enable TLS (auto-configured with `rustls`)
- Use strong token auth (256-bit minimum)
- Rate limiting on auth failures

**User action required:** If you expose the gateway port beyond localhost (LAN, public), you MUST configure authentication. FrankClaw will refuse to bind to `0.0.0.0` without auth enabled (hard enforcement).

---

#### 3. Model Provider API Keys (MUST be sent to provider APIs)

**What:** OpenAI, Anthropic, Google API keys are sent in HTTP headers to the respective APIs.

**Risk:** Key compromise means unauthorized API usage (financial and data exposure).

**Mitigations available:**
- Keys encrypted at rest (master key required at startup)
- Keys never logged (redaction in tracing layer)
- Per-provider spending limits (configured at provider dashboard)
- Auth profile cooldowns on failure

**What FrankClaw cannot do:** The API key is the authentication mechanism these providers require. We cannot proxy, hash, or otherwise obscure it — the provider must receive the actual key. Set spending limits at the provider level.

---

#### 4. WhatsApp (Baileys) Session State

**What:** WhatsApp Web authentication requires maintaining an active browser-like session with persistent credentials.

**Risk:** The session state grants full access to the WhatsApp account. If exfiltrated, an attacker can read and send messages as you.

**Mitigations available:**
- Session files encrypted at rest
- File permissions restricted to owner (`0o600`)
- Session file directory excluded from backups by default

**User action required:** This is inherently the most sensitive channel. The WhatsApp Web protocol is reverse-engineered (Baileys) and may break. WhatsApp may detect and ban automated usage. Use at your own risk. **Do not use with business-critical WhatsApp accounts.**

---

#### 5. Signal (signal-cli) Subprocess

**What:** signal-cli runs as a subprocess with its own credential store and exposes an HTTP API on localhost.

**Risk:** The signal-cli HTTP API has no authentication. Any process on localhost can send Signal messages.

**Mitigations available:**
- Bind signal-cli to `127.0.0.1` only (default)
- Use a random port (allocated at startup)
- Consider running signal-cli in a separate container/namespace

**User action required:** Do not expose the signal-cli HTTP port to the network. If running in Docker, do not map this port. FrankClaw binds it to a random localhost port by default but **cannot prevent other local processes from connecting**.

---

#### 6. Webhook Endpoints (MUST accept external HTTP)

**What:** Some channels (Slack, Telegram, Discord) send events via webhook POST to your server.

**Risk:** Webhook endpoints must be publicly accessible. Attackers can forge webhook payloads.

**Mitigations available:**
- Signature verification per platform (Telegram: secret token, Slack: signing secret, Discord: Ed25519)
- Request body size limits
- Rate limiting per source IP
- Webhook-specific auth tokens

**User action required:** Always configure webhook signature verification. FrankClaw will warn (but not block) if a webhook channel is enabled without signature verification configured. Platform-specific replay protection is enabled by default where supported.

---

#### 7. Media Files in Sandbox Mode

**What:** When the agent runs in a Docker/Podman sandbox, media files must be accessible to the container.

**Risk:** Files shared into the container are accessible to whatever code the agent executes.

**Mitigations available:**
- Read-only bind mounts where possible
- Separate media directory per sandbox invocation
- Automatic cleanup after sandbox exits
- File size limits enforced before sharing

**User action required:** Understand that any file shared into a sandbox is potentially accessible to untrusted code. Do not share directories containing sensitive files. FrankClaw uses a dedicated, ephemeral media directory by default.

---

#### 8. Memory Vector Database (Embeddings reveal content)

**What:** Text content is converted to vector embeddings for semantic search. Embeddings are mathematical representations that partially encode the original text content.

**Risk:** While embeddings cannot be trivially reversed to exact text, they leak semantic information. An attacker with access to the embedding vectors could determine topic similarity, perform membership inference, or use model inversion techniques.

**Mitigations available:**
- Text content encrypted at rest (vectors are not — they must be searchable)
- Database file permissions restricted to owner
- Optional: use local embedding model (Ollama) to avoid sending content to external APIs

**User action required:** If you use cloud embedding providers (OpenAI, Google), your memory content is sent to their APIs for embedding. Use local models (Ollama) if you need full privacy. The vector representations in LanceDB are inherently unencryptable if you want search to work.

---

#### 9. Config File and Environment Variables

**What:** The config file and `.env` may contain API keys, tokens, and other secrets.

**Risk:** Any process with read access to these files can extract all credentials.

**Mitigations available:**
- Encrypted config mode (master passphrase required at startup)
- Environment variables cleared from process memory after reading
- File permissions enforced (`0o600`)
- `$ref` secret references for external secret managers

**User action required:** Never commit `.env` or config files to version control. Use encrypted config mode for production deployments. Consider integrating with external secret managers (HashiCorp Vault, AWS Secrets Manager) via the `$ref` mechanism.

---

## Part 6: Implementation Phases

### Phase 1: Foundation (Weeks 1-4)
- `frankclaw-core`: All shared types, traits, error hierarchy
- `frankclaw-cli`: Basic CLI skeleton with `clap`
- `frankclaw-gateway`: Axum server, WS handler, auth middleware
- `frankclaw-sessions`: SQLite store, basic CRUD, encryption
- Config loading (JSON5/YAML) with validation

### Phase 2: Model Providers (Weeks 5-7)
- `frankclaw-models`: OpenAI, Anthropic, Ollama adapters
- Streaming response handling (SSE parsing)
- Failover chain with cooldowns
- Token counting and cost tracking

### Phase 3: Core Channels (Weeks 8-12)
- `frankclaw-channels`: Telegram, Discord, Slack (the big three)
- Channel plugin trait finalized
- Inbound/outbound message pipeline
- Draft streaming and chunking

### Phase 4: Agent Runtime (Weeks 13-16)
- `frankclaw-agents`: ACP session manager
- Tool system (browser, cron, canvas)
- Skill loader
- Sandbox integration (Bubblewrap, Docker)

### Phase 5: Extended Channels + Memory (Weeks 17-20)
- Signal, WhatsApp, IRC, Matrix, Web channel
- `frankclaw-memory`: LanceDB integration, embedding providers
- `frankclaw-cron`: Full cron service
- `frankclaw-media`: Media pipeline with SSRF protection

### Phase 6: Plugin SDK + Polish (Weeks 21-24)
- `frankclaw-plugin-sdk`: Dynamic plugin loading
- Control UI serving
- Device node pairing
- Comprehensive test suite
- Security audit

---

## Part 7: Critical Rust Best Practices

### Error Handling
```rust
// Unified error type with thiserror
#[derive(Debug, thiserror::Error)]
pub enum FrankClawError {
    #[error("authentication failed: {reason}")]
    AuthFailed { reason: String },

    #[error("channel {channel} error: {source}")]
    Channel { channel: ChannelId, #[source] source: ChannelError },

    #[error("session not found: {key}")]
    SessionNotFound { key: SessionKey },

    #[error("model provider error: {source}")]
    ModelProvider { #[source] source: ModelError },

    // ... all variants explicit, no catch-all
}
```

### No Unwrap in Production Code
```rust
// NEVER: value.unwrap()
// ALWAYS: value.map_err(|e| FrankClawError::from(e))?
// Or for truly impossible states: value.expect("invariant: X is always Some after init")
```

### Structured Concurrency
```rust
// Use tokio::select! for cancellation-safe operations
// Use JoinSet for managing groups of spawned tasks
// Always propagate CancellationToken through the call chain
// Never spawn unbounded tasks — use semaphores
```

### Secret Handling
```rust
use secrecy::{SecretString, ExposeSecret};
use zeroize::Zeroize;

// Secrets are:
// 1. Wrapped in SecretString (Debug prints "[REDACTED]")
// 2. Zeroed from memory on drop
// 3. Never serialized by default (custom Serialize skips them)
// 4. Compared in constant time
```

### Dependency Policy
- Every direct dependency must be in `cargo-deny` allowlist
- No `unsafe` in FrankClaw code (use `#![forbid(unsafe_code)]`)
- `unsafe` in dependencies audited via `cargo-geiger`
- MSRV (Minimum Supported Rust Version) pinned and tested in CI
- `cargo-audit` runs on every PR
