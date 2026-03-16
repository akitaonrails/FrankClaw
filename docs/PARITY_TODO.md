# FrankClaw Parity TODO

This file tracks the remaining distance between FrankClaw and the broader OpenClaw feature surface.
It should stay current as features land, are deferred, or are explicitly dropped.

**Last verified**: 2026-03-16 — systematic directory-by-directory audit of OpenClaw `src/` (~192k LOC
across ~2,864 non-test TypeScript files) against FrankClaw (~37k LOC across 13 Rust crates).
IronClaw feature adoption complete (12 features across 4 phases).
OpenClaw parity phase complete (14 features: web UI enrichment + backend features).

## Current Position

FrankClaw has been audited against OpenClaw's battle-tested implementation.
See `AUDIT_PLAN.md` for the full audit results across all 14 components.
It now has a working hardened core with:

- inbound/outbound assistant loop
- session persistence and optional transcript encryption
- provider failover with circuit breaker, retry with exponential backoff + jitter
- smart model routing (13-dimension complexity scorer)
- response caching (SHA-256 keyed LRU with TTL)
- cost tracking with daily budget guards
- credential leak detection (12 patterns)
- extended thinking for reasoning models
- MCP client (stdio + HTTP transports)
- tunnel support (Cloudflare, ngrok, custom)
- event-driven routine triggers (cron, message pattern, system events, manual)
- job state machine with self-repair
- interactive REPL (`frankclaw chat`)
- DM pairing and stricter channel defaults
- rich 8-tab web console with dark mode, tool approval, usage analytics, logs viewer
- cron reuse with gateway RPC wiring
- signed webhooks with JSON path extraction, templates, rate limiting
- bounded tool execution
- local Canvas host
- operator onboarding and install helpers

FrankClaw now covers the **core message-to-model flow** comprehensively with a rich
web console, memory/RAG, multi-provider media understanding, webhook transforms,
and full hook wiring. The remaining gap is primarily long-tail channel breadth and
voice — not architecture, runtime intelligence, or the transport layer.

## Implemented Core and Surfaces

- [x] Runtime-backed chat flow
- [x] Session persistence, pruning, encryption support
- [x] WebSocket gateway control plane for core methods
- [x] Local browser console UI
- [x] Pairing and inbound policy hardening
- [x] Cron execution through shared runtime
- [x] Signed webhook ingestion with replay protection
- [x] Read-only and bounded model-driven tools
- [x] Local Canvas host surface with revision conflict detection
- [x] Operator health, remote exposure, onboarding, and systemd helpers
- [x] Normalized inbound media placeholders on supported channels
- [x] Chromium-backed browser session tools (`open`, `extract`, `snapshot`)
- [x] Selector-based browser actions (`click`, `type`, `wait`, `press`)
- [x] Browser session visibility and close control (`sessions`, `close`)
- [x] Provider SSE streaming for OpenAI/Anthropic/Ollama
- [x] Rich web console with 8 tabs (Connect, Chat, Canvas, System, Usage, Agents, Cron, Logs)
- [x] Dark/light mode toggle with system preference detection and FOUC prevention
- [x] WebSocket auto-reconnect with ping keepalive (survives proxy/tunnel idle timeouts)
- [x] Tool approval inline UI with Allow Once/Always/Deny controls
- [x] Image paste upload in chat
- [x] Usage analytics with CSV export
- [x] Agent management (read-only config view)
- [x] Logs viewer with level filters, text search, auto-scroll, ring buffer
- [x] Focus mode (full viewport chat)
- [x] Resizable markdown sidebar for tool output
- [x] Cron job management (list, add, remove, run now) via gateway RPC
- [x] Webhook JSON path extraction, templates, per-mapping rate limiting and concurrency
- [x] Memory/RAG with SQLite FTS5, embedding providers, file sync
- [x] Hook lifecycle wiring in runtime (message.received/sent, tool.before/after)
- [x] Multi-provider media understanding with fallback chain (OpenAI, Anthropic, Ollama vision + Whisper)
- [x] OpenAI-compatible API (`/v1/chat/completions`, `/v1/models`) for drop-in client support
- [x] Model-aware failover routing (skips providers that don't serve the requested model)
- [x] Canvas SVG/HTML/Markdown rendering in web console
- [x] GitHub Copilot model provider

## Implemented Channels

- [x] Web
- [x] Telegram
- [x] Discord
- [x] Slack
- [x] Signal
- [x] WhatsApp Cloud API

## Missing or Partial vs OpenClaw

### Agent Intelligence Layer

These are the core "brain" features that make OpenClaw's agent loop sophisticated:

- [x] **Context Engine** — Sliding window compaction with token estimation, message pruning,
  tool pairing repair, and summary marker insertion. (`frankclaw-runtime/src/context.rs`)

- [x] **Context Compaction** — Automatic context window management with safety margins,
  per-message overhead estimation, and orphaned tool result cleanup.

- [x] **Subagent System** — Hierarchical agent spawning with depth limits, concurrency control,
  lifecycle tracking (pending → running → completed/failed/killed), push-based completion
  notification, and system prompt context injection. (`frankclaw-runtime/src/subagent.rs`)

- [x] **Auto-Reply Command System** — Prefix-based command detection (`/cmd`), alias resolution,
  inline directive extraction (`/think`, `/model`), help generation, and dispatch pipeline
  with bypass-model capability. (`frankclaw-runtime/src/commands.rs`)

- [x] **System Prompt Construction** — Dynamic system prompt assembly from identity, user prompt,
  tool listing, skills, safety rules, and runtime metadata. (`frankclaw-runtime/src/lib.rs`)

### Multimodal & Content Understanding

- [x] **Media Understanding** — Multi-provider vision (OpenAI, Anthropic, Ollama) and audio
  transcription (Whisper), fallback chain, media kind classification, attachment processing
  pipeline with size limits and graceful error handling, `audio_transcribe` tool, configurable
  via `MediaUnderstandingConfig`. (`frankclaw-media/src/understanding.rs`,
  `frankclaw-tools/src/audio.rs`, `frankclaw-core/src/media.rs`)

- [x] **Link Understanding** — SSRF-safe URL extraction from messages with deduplication,
  markdown link stripping, and private IP/hostname blocking. (`frankclaw-core/src/links.rs`)

- [x] **TTS (Text-to-Speech)** — **SKIPPED**: voice output is a gimmick, not core functionality.

### Extensibility & Hooks

- [x] **Hooks System** — Event-driven hook registry with 6 event types (command, session, agent,
  gateway, message, tool), async fire-and-forget execution, general and specific event matching,
  30s timeout per handler, typed event constructors. Wired into runtime: message.received/sent,
  tool.before/after. (`frankclaw-core/src/hooks.rs`, `frankclaw-runtime/src/lib.rs`)

- [x] **Gmail Integration** — **SKIPPED**: complex Google Pub/Sub integration for a niche channel.

- [x] **Skills System** — Workspace-loaded skill manifests with validation, capability-based
  tool access control, and prompt injection. (`frankclaw-plugin-sdk/src/lib.rs`)

- [x] **ACP (Agent Client Protocol)** — JSON-RPC 2.0 over NDJSON (stdin/stdout) with session
  management (DashMap, 24h TTL, LRU eviction), rate limiting, streaming prompt responses.
  Methods: initialize, newSession, loadSession, prompt, listTools, callTool.
  (`frankclaw-gateway/src/acp.rs`, `acp_transport.rs`; CLI `frankclaw acp`)

### Runtime & Execution

- [x] **Sandboxed Agent Runtime** — **DONE**: Optional `ai-jail` integration (bubblewrap +
  landlock) instead of Docker. Lighter weight, per-command spawning. Set
  `FRANKCLAW_SANDBOX=ai-jail` or `ai-jail-lockdown`. Security audit reports sandbox status.
  Works alongside bash policy allowlist as complementary layers.

- [x] **Bash Tools** — Shell command execution with timeout enforcement, output truncation,
  working directory support, and configurable security policy (deny-all, allow-all, or
  binary allowlist). (`frankclaw-tools/src/bash.rs`)

- [x] **Model Catalog & Discovery** — Static catalog with known metadata (context windows, costs,
  capabilities) for OpenAI and Anthropic models. Enrichment fallback for unknown models with
  conservative API-specific defaults. (`frankclaw-models/src/catalog.rs`)

- [x] **Auth Profile Rotation** — Multi-key per provider with round-robin selection, exponential
  backoff on failure, automatic recovery on cooldown expiry, and provider-level key management.
  (`frankclaw-core/src/api_keys.rs`)

- [x] **Memory/RAG System** — SQLite FTS5 + cosine vector search with hybrid scoring,
  embedding providers (OpenAI, Ollama) with SHA-256 caching, paragraph-based chunking
  with line tracking, file syncer with content hash change detection.
  (`frankclaw-memory/src/store.rs`, `embedding.rs`, `chunking.rs`, `sync.rs`)

### Channel Features

- [x] **Polls** — **SKIPPED**: marginal value channel-specific feature.

- [x] **WhatsApp Web** — **SKIPPED**: Baileys/WA Web Socket is fragile; Cloud API covers the use case.

### Secrets & Security

- [x] **Security Audit & Secrets Check** — `frankclaw audit` with severity-rated findings
  (CRIT/HIGH/MED/LOW/INFO) across 7 categories: auth posture, inline secrets, missing
  env vars, encryption status, network exposure, channel policies (group gating, webhook
  signatures), tool policies (bash allowlist, browser mutations), SSRF protection, and
  file permission audits. CI-friendly exit code 1 on critical/high findings.
  (`frankclaw-cli/src/main.rs`)

### Operator Experience

- [x] **Daemon Management** — `frankclaw start/stop/status` with PID file tracking, log
  redirection, graceful SIGTERM shutdown with SIGKILL fallback. Also retains systemd unit
  generation for production deployments.

- [x] **Interactive Setup Wizard** — `frankclaw setup` with guided provider selection
  (OpenAI/Anthropic/Ollama), API key env var configuration, channel selection (6 channels),
  port choice, session encryption toggle, and automatic gateway token generation.

- [x] **Doctor Diagnostics** — comprehensive `frankclaw doctor` covering system info, config
  validation, state directory health, SQLite DB integrity, port availability, async provider
  connectivity checks, Unix file/directory permission audits, channel status, and security
  posture with structured PASS/WARN/FAIL/INFO output.

### Rich Channel Behavior (Previously Checked — Done)

- [x] Rich attachment/media handling across supported channels
- [x] Broader edit support beyond Telegram
- [x] Delete support where platforms allow it
- [x] Shared outbound text normalization and reply-safe formatting
- [x] Channel-specific streaming or pseudo-streaming delivery
- [x] Explicit group allowlist routing on supported group-capable channels
- [x] Better reply-tag semantics across supported channels
- [x] Better WhatsApp-specific behavior
- [x] Broader platform-specific retry/backoff semantics

### Canvas Depth (Previously Checked — Done)

- [x] Structured Canvas document model with revision conflict detection
- [x] Session-linked Canvas workflows
- [x] Incremental Canvas patches
- [x] Multiple canvases or per-session canvases
- [x] Safer agent-driven UI blocks/components
- [x] Snapshot/export flows
- [x] A2UI-style richer host capabilities

### Tool Depth (Previously Checked — Done)

- [x] Browser automation runtime with CDP timeout and SSRF guards
- [x] Browser session/profile management with dead session recovery
- [x] Visual/browser snapshots
- [x] Safer action model for clicks/forms/navigation
- [x] Tool approvals for higher-risk tool families
- [x] More first-party tools beyond session inspection
- [x] LLM tool suite: web_fetch, web_search, sessions_list, sessions_history,
  file_read, file_write, file_edit, message_send, cron_list, cron_add,
  cron_remove, config_get, agents_list, memory_get, memory_search,
  image_describe, audio_transcribe, pdf_read (18 tools)
- [x] Better tool tracing and operator visibility

### Test Coverage

- [x] Integration coverage across supported channels
- [x] Gateway-path coverage for authenticated web media upload/inbound flows
- [x] End-to-end coverage for operator flows
- [x] External-API contract fixtures for supported channels
- [x] Failure-path tests for provider failover and retries
- [x] Coverage for Canvas RPC/UI behavior
- [x] Coverage for onboarding/install helpers
- [x] Regression-focused tests for delivery metadata and session rewrites
- [x] Live smoke coverage against real external platforms (`frankclaw-models/tests/smoke.rs`)
- [x] Media-specific failure-path coverage for partial multi-attachment delivery

### Still Missing OpenClaw Channel Breadth

- [ ] Google Chat
- [ ] BlueBubbles / iMessage
- [ ] IRC
- [ ] Microsoft Teams
- [ ] Matrix
- [ ] Feishu
- [ ] LINE
- [ ] Mattermost
- [ ] Nextcloud Talk
- [ ] Nostr
- [ ] Synology Chat
- [ ] Tlon
- [ ] Twitch
- [ ] Zalo
- [ ] Zalo Personal
- [ ] Companion nodes and apps
- [ ] Voice

## Priority Tiers

### Tier 1 — Core Intelligence (high impact, needed for competitive parity)

1. ~~Context engine with compaction~~ ✅
2. ~~Media understanding pipeline~~ ✅
3. ~~System prompt construction~~ ✅
4. ~~Link understanding~~ ✅

### Tier 2 — Advanced Agent Capabilities

5. ~~Subagent system~~ ✅
6. ~~Auto-reply command system~~ ✅
7. ~~Model catalog/discovery~~ ✅
8. ~~Auth profile rotation~~ ✅

### Tier 3 — Extensibility

9. ~~Hooks system~~ ✅
10. ~~Skills system~~ ✅ (already implemented in plugin-sdk)
11. ~~ACP protocol~~ ✅ — JSON-RPC 2.0 over NDJSON (`frankclaw acp`)
12. ~~Bash tools with sandboxing~~ ✅

### Tier 4 — Operator Experience

13. ~~Doctor diagnostics~~ ✅ — comprehensive `frankclaw doctor` with system info, config
    validation, state dir health, SQLite integrity, port availability, provider connectivity,
    file permission audits, channel status, and security posture.
14. ~~Interactive setup wizard~~ ✅ — `frankclaw setup` guides through provider selection
    (OpenAI/Anthropic/Ollama), API key config, channel selection, port, encryption.
15. ~~Process management~~ ✅ — `frankclaw start/stop` with PID file tracking, log redirection,
    graceful shutdown (SIGTERM → SIGKILL fallback), stale PID detection.

### Tier 4 — Skipped (low value or excessive effort)

- ~~TTS~~ — voice output is a gimmick, not core functionality
- ~~Polls~~ — channel-specific feature, marginal value
- ~~WhatsApp Web~~ — Baileys/WA Web Socket is fragile and complex; Cloud API covers the use case
- ~~Gmail integration~~ — complex Google Pub/Sub integration for a niche channel
- ~~Device pairing~~ — Bonjour/mDNS/Tailscale discovery is over-engineered for a self-hosted tool
- ~~Auto-update~~ — users can use their package manager or pull from git
- ~~Markdown IR~~ ✅ — `MarkdownIR` with pulldown-cmark parser and ANSI SGR rendering (`frankclaw-runtime/src/markdown.rs`)
- ~~i18n~~ — ✅ Implemented: 9 locales via `FRANKCLAW_LANG` (en, pt-BR, pt-PT, es, fr, de, it, ja, ko)
- ~~Browser profiles~~ ✅ — Named CDP profiles with port allocation (18800-18899), color cycling (`frankclaw-tools/src/browser_profiles.rs`)
- ~~Batch embedding providers~~ ✅ — Gemini (768D) and Voyage AI (1024D) providers with 100-text batching (`frankclaw-memory/src/embedding.rs`)
- ~~Plugin management~~ ✅ — Manifest parsing, filesystem discovery, enable/disable lifecycle (`frankclaw-plugin-sdk/src/manifest.rs`, `discovery.rs`, `lifecycle.rs`; CLI `frankclaw plugin`)
- ~~TUI~~ ✅ — Full-screen ratatui TUI with chat log, input area, status bar, streaming, slash commands (`frankclaw-cli/src/tui.rs`; launched via `frankclaw chat --tui`)
- ~~Wizard improvements~~ — **Not needed**: current `frankclaw setup` covers the critical path (provider, channel, auth)
- ~~Secrets audit credential matrix~~ — **Not needed**: FrankClaw uses single config + env vars, not OpenClaw's multi-file auth profile system

### IronClaw-Derived Features (Adopted)

These features were adopted from [IronClaw](https://github.com/nearai/ironclaw) (MIT OR Apache-2.0)
in 4 phases. See the plan file for full analysis of 18 IronClaw features, 12 adopted, 6 skipped.

- [x] **Circuit breaker + retry** — Per-provider health tracking (Closed→Open→HalfOpen),
  exponential backoff with jitter, configurable thresholds. (`frankclaw-models/src/circuit_breaker.rs`, `retry.rs`)
- [x] **Credential leak detection** — 12 regex patterns scan LLM and tool output for API keys,
  tokens, and secrets. (`frankclaw-models/src/leak_detector.rs`)
- [x] **LLM response caching** — In-memory SHA-256 keyed LRU cache with configurable TTL,
  bypassed for streaming. (`frankclaw-models/src/cache.rs`)
- [x] **Cost tracking & budget guards** — Per-model token cost tables, daily budget with
  warn-at-80%/block-at-100%. (`frankclaw-models/src/costs.rs`, `cost_guard.rs`)
- [x] **Extended thinking** — `thinking_budget` on CompletionRequest, Anthropic provider support.
- [x] **REPL channel** — `frankclaw chat` with rustyline, streaming, slash commands, tab completion,
  history persistence, i18n. (`frankclaw-cli/src/repl.rs`)
- [x] **Smart model routing** — 13-dimension complexity scorer with pattern overrides, tier hints,
  multi-dimensional boost. (`frankclaw-models/src/routing.rs`)
- [x] **MCP client** — JSON-RPC 2.0 client with stdio/HTTP transports, tool wrapping, risk level
  mapping from MCP annotations. (`frankclaw-tools/src/mcp/`)
- [x] **Lifecycle hooks** — Already existed in FrankClaw with 5 event categories. SKIPPED.
- [x] **Tunnel support** — Cloudflare Tunnel, ngrok, custom commands with URL extraction from
  process output, env-based configuration. (`frankclaw-gateway/src/tunnel.rs`)
- [x] **Job state machine** — 8 states with validated transitions, self-repair with max attempts,
  token budget tracking. (`frankclaw-cron/src/job.rs`)
- [x] **Event trigger system** — 4 trigger types (Cron/Event/SystemEvent/Manual), guardrails
  (cooldown, max concurrent, dedup), lightweight vs full-job actions. (`frankclaw-cron/src/triggers.rs`)

Skipped IronClaw features (don't fit architecture or lower priority):
- WASM tool sandbox (ai-jail covers this)
- Docker container execution (contradicts zero-external-deps philosophy)
- Full web dashboard UI (backend-first approach)
- OS keychain integration (encrypted config is sufficient)
- Session threading (flat transcript model is simpler)
- Workspace-based memory (LanceDB integration planned separately)

### Deferred / Lower Priority

- Wider long-tail channel parity (Google Chat, iMessage, IRC, Teams, Matrix, etc.)
- Companion node/app surfaces
- Voice
- Distro-specific installers
