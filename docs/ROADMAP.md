# FrankClaw Roadmap

## Goal

Ship a security-hardened Rust assistant gateway that delivers the core OpenClaw flow without inheriting its sprawl:

1. Receive messages on supported channels.
2. Resolve agent and session safely.
3. Call configured model providers with failover.
4. Persist encrypted transcripts.
5. Deliver replies back to the originating surface.

## v1 Scope

- First-party channels: `web`, `telegram`
- First-party providers: `openai`, `ollama`, `anthropic`
- Gateway transport: local WebSocket control plane
- Session store: SQLite
- Security defaults:
  - loopback bind
  - auth required for non-loopback
  - DM pairing / allowlist before open delivery
  - per-channel-peer session scoping
  - transcript encryption with master key support
  - no exec/browser/tool runtime in v1

## Non-Goals (Original v1)

Many original non-goals were subsequently implemented:

- ~~Canvas~~ — ✅ Implemented (structured document model with revision conflict detection)
- companion mobile / desktop nodes
- ~~browser automation~~ — ✅ Implemented (CDP-based, 9 tools)
- wide channel parity (7 native channels is sufficient)
- dynamic plugin loading
- ~~onboarding wizard parity~~ — ✅ Implemented (`frankclaw setup`, `onboard`, `doctor`)
- ~~webhook ecosystem~~ — ✅ Implemented (signed webhook ingestion with replay protection)
- ~~skills runtime~~ — ✅ Implemented (workspace-loaded skill manifests)

## Milestones

### M0: Correctness and Safety Baseline

- [x] Add tracked roadmap
- [x] Fix auth config serialization / loading
- [x] Fix auth ingress parsing for configured auth modes
- [x] Make `frankclaw check` validate actual invariants
- [x] Redact sensitive config output

Acceptance:

- Invalid auth/provider config fails `check`
- Token auth can be loaded from config
- `config` output never prints secrets

### M1: Runtime Vertical Slice

- [x] Add `frankclaw-runtime`
- [x] Load configured providers from config
- [x] Add transcript-backed `chat.send`
- [x] Return real `models.list`
- [x] Return configured `channels.list`

Acceptance:

- A WebSocket client can send a chat request and receive a model reply
- The reply is persisted to the session transcript

### M2: Channel Ingress

- [x] Start `web` as a first-party local channel
- [x] Start `telegram` with runtime-backed inbound processing
- [x] Route inbound channel messages through the same turn executor
- [x] Persist reply metadata needed for retries and edits

Acceptance:

- Telegram inbound -> model -> reply works end-to-end
- Web channel inbound -> model -> reply works end-to-end

### M3: Ingress Hardening

- [x] DM pairing / allowlist enforcement
- [x] Group mention gating by default
- [x] Per-channel-peer session policy wired into runtime
- [x] Inbound message size limits and normalization
- [x] Outbound retry / rate-limit behavior

Acceptance:

- Unknown DM senders are blocked by default
- Group messages do not execute unless explicitly permitted

### M4: Session and Secret Hardening

- [x] Load a master key from CLI / environment
- [x] Enable transcript encryption in real startup
- [x] Wire session maintenance / pruning
- [x] Add structured security audit logs

Acceptance:

- Transcripts are encrypted at rest when configured
- Pruning runs without losing active sessions

### M5: Cron Reuse

- [x] Persist cron jobs
- [x] Execute cron through the shared turn executor
- [x] Add strict session targeting rules

Acceptance:

- Scheduled prompts use the same model/session path as interactive chat

### M6: Operator Surface

- [x] Add `doctor`
- [x] Add `message send`
- [x] Add `pairing list|approve`
- [x] Add `sessions list|get|reset`
- [x] Add `models list`

Acceptance:

- Operators can inspect and drive the core system without raw WS calls

## Release Gate

FrankClaw is ready for a v1 parity claim only when all of the following are true:

- `web` and `telegram` complete the full reply loop
- provider failover works
- session history survives restart
- pairing works
- unsupported OpenClaw surfaces are explicitly out of scope, not half-exposed
- unit tests cover the supported flow today; integration and e2e coverage are still the next release gate
