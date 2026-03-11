# FrankClaw Parity TODO

This file tracks the remaining distance between FrankClaw and the broader OpenClaw feature surface.
It should stay current as features land, are deferred, or are explicitly dropped.

**Last verified**: 2026-03-11 — systematic directory-by-directory audit of OpenClaw `src/` (~192k LOC
across ~2,864 non-test TypeScript files) against FrankClaw (~27k LOC across 13 Rust crates).

## Current Position

FrankClaw has been audited against OpenClaw's battle-tested implementation.
See `AUDIT_PLAN.md` for the full audit results across all 14 components.
It now has a working hardened core with:

- inbound/outbound assistant loop
- session persistence and optional transcript encryption
- provider failover
- DM pairing and stricter channel defaults
- local console UI
- cron reuse
- signed webhooks
- bounded tool execution
- local Canvas host
- operator onboarding and install helpers

FrankClaw covers the **core message-to-model flow** well but is missing many
of OpenClaw's advanced subsystems. The gap is primarily in runtime intelligence,
extensibility, and multimodal capabilities — not the transport/plumbing layer.

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

- [ ] **Auto-Reply Command System** — Structured command dispatch
  OpenClaw has a full command detection, registry, and dispatch pipeline with heartbeat,
  inbound debounce, group activation, thinking mode management, model directives, and block
  streaming. FrankClaw has a simpler direct-to-model flow.

- [x] **System Prompt Construction** — Dynamic system prompt assembly from identity, user prompt,
  tool listing, skills, safety rules, and runtime metadata. (`frankclaw-runtime/src/lib.rs`)

### Multimodal & Content Understanding

- [x] **Media Understanding** — Vision description via OpenAI-compatible vision API, audio
  transcription via Whisper API, media kind classification, attachment processing pipeline
  with size limits and graceful error handling. (`frankclaw-media/src/understanding.rs`,
  `frankclaw-core/src/media.rs`)

- [x] **Link Understanding** — SSRF-safe URL extraction from messages with deduplication,
  markdown link stripping, and private IP/hostname blocking. (`frankclaw-core/src/links.rs`)

- [ ] **TTS (Text-to-Speech)** — Voice output
  OpenClaw supports multiple TTS providers: OpenAI TTS, ElevenLabs, Edge TTS (Microsoft),
  Sherpa ONNX (local). Per-channel auto mode, text summarization before synthesis, markdown
  stripping. FrankClaw has no TTS.

### Extensibility & Hooks

- [ ] **Hooks System** — Event-driven extensibility
  OpenClaw has a full hook system with event types (command, session, agent, gateway, message),
  fire-and-forget hooks, bundled hooks, message hook mappers, frontmatter parsing, workspace
  hooks, hook installer/loader. FrankClaw has webhooks but no general hook/plugin event system.

- [ ] **Gmail Integration** — Email as inbound channel
  OpenClaw integrates Gmail via Google Pub/Sub with Tailscale tunneling, label filtering,
  push token auth, configurable polling. FrankClaw has no email integration.

- [ ] **Skills System** — Installable agent skills
  OpenClaw has installable/downloadable skills, workspace scanning, bundled allowlist,
  skill status tracking, tar extraction. FrankClaw has no skills concept.

- [ ] **ACP (Agent Client Protocol)** — Standard agent protocol
  OpenClaw implements ACP for agent sessions with persistent bindings, session mapping,
  policy enforcement, event mapping, secret file support, rate limiting. FrankClaw has
  its own WebSocket protocol but no ACP.

### Runtime & Execution

- [ ] **Sandboxed Agent Runtime** — Docker-based execution
  OpenClaw has full Docker container sandboxing with path policies, media path mapping,
  tool policy enforcement, per-agent sandbox config, workspace-only mode. FrankClaw has
  no sandboxing.

- [ ] **Bash Tools** — Shell command execution
  OpenClaw has bash tool execution with PTY support, process registry, approval requests,
  background jobs, abort handling, script preflight, send-keys. FrankClaw has browser
  tools but no shell execution tools.

- [x] **Model Catalog & Discovery** — Static catalog with known metadata (context windows, costs,
  capabilities) for OpenAI and Anthropic models. Enrichment fallback for unknown models with
  conservative API-specific defaults. (`frankclaw-models/src/catalog.rs`)

- [ ] **Auth Profile Rotation** — Multi-profile provider auth
  OpenClaw supports multiple auth profiles per provider with cooldown auto-expiry, round-robin
  ordering, runtime snapshots, failure tracking. FrankClaw has single-key-per-provider auth.

- [ ] **Vector Memory Backend** — Persistent memory search
  OpenClaw's context engine supports vector memory for long-term knowledge retrieval.
  FrankClaw has a `frankclaw-memory` crate with traits defined but no backend.

### Channel Features

- [ ] **Polls** — Cross-channel poll creation
  OpenClaw supports creating polls on Telegram (open_period) and Discord (duration_hours)
  with question/options normalization, multi-selection, channel-specific limits. FrankClaw
  has no poll support.

- [ ] **WhatsApp Web** — Baileys/WA Web Socket channel
  OpenClaw has a separate WhatsApp Web channel via Baileys (WA Web socket), distinct from
  the Cloud API. FrankClaw only has Cloud API.

### Secrets & Security

- [ ] **Secrets Management** — Full secrets audit and management
  OpenClaw has secret audit, auth profile scanning, credential matrix, JSON pointer
  resolution, configure plan, runtime auth collectors, provider env var mapping.
  FrankClaw has `SecretString` wrapping but no secrets audit/management CLI.

- [ ] **Security Audit** — Automated config security scanning
  OpenClaw has automated security scanning: dangerous config flags, mutable allowlist
  detectors, skill scanner, safe regex validation, external content analysis, tool
  policy audit. FrankClaw has SSRF protection and input validation but no automated
  security scanning.

### Operator Experience

- [ ] **Full Daemon Management** — Cross-platform service management
  OpenClaw has systemd unit generation, launchd plist generation, Windows schtasks,
  service audit, runtime binary discovery, service environment management. FrankClaw
  has basic systemd unit output.

- [ ] **Interactive Wizard/Onboarding** — Guided setup
  OpenClaw has an interactive wizard with Clack prompts for gateway config, secret input,
  completion flows, channel setup. FrankClaw has CLI config commands but no interactive
  wizard.

- [ ] **Doctor Diagnostics** — Deep health checks
  OpenClaw's `doctor` command covers config analysis, legacy migration, memory search,
  session locks, state integrity, workspace status, bootstrap size, daemon flows, security
  checks. FrankClaw has basic `health` checks.

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
- [ ] Live smoke coverage against real external platforms
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
6. Auto-reply command system (richer interaction model)
7. ~~Model catalog/discovery~~ ✅
8. Auth profile rotation (production reliability)

### Tier 3 — Extensibility

9. Hooks system (foundation for extensibility)
10. Skills system (community contribution)
11. ACP protocol (standard interop)
12. Bash tools with sandboxing (powerful but needs security care)

### Tier 4 — Nice-to-Have

13. TTS (voice output)
14. Polls (channel-specific feature)
15. WhatsApp Web (niche alternative to Cloud API)
16. Gmail integration (niche hook)
17. Full daemon management across platforms (launchd, schtasks beyond systemd)
18. Interactive wizard/onboarding
19. Deep doctor diagnostics
20. i18n / multi-locale support (OpenClaw has EN, ZH, PT-BR, DE, ES)
21. Device pairing with network discovery (Bonjour/mDNS, Tailscale)
22. Markdown IR with channel-specific rendering (WhatsApp, LINE formatters)
23. Process management with lane-based task queueing and concurrency control
24. Auto-update system with release channels

### Deferred / Lower Priority

- Wider long-tail channel parity (Google Chat, iMessage, IRC, Teams, Matrix, etc.)
- Companion node/app surfaces
- Voice
- Distro-specific installers
- Secrets audit CLI
- Full TUI (FrankClaw has basic console; OpenClaw has full interactive client with session tabs, token display, syntax highlighting)
