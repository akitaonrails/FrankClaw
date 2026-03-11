# FrankClaw Parity TODO

This file tracks the remaining distance between FrankClaw and the broader OpenClaw feature surface.
It should stay current as features land, are deferred, or are explicitly dropped.

## Current Position

FrankClaw is no longer just a foundation rewrite.
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

FrankClaw is still not close to full OpenClaw breadth.
The main remaining gap is feature surface, not the core message-to-model flow.

## Implemented Core and Surfaces

- [x] Runtime-backed chat flow
- [x] Session persistence, pruning, encryption support
- [x] WebSocket gateway control plane for core methods
- [x] Local browser console UI
- [x] Pairing and inbound policy hardening
- [x] Cron execution through shared runtime
- [x] Signed webhook ingestion
- [x] Read-only and bounded model-driven tools
- [x] Local Canvas host surface
- [x] Operator health, remote exposure, onboarding, and systemd helpers
- [x] Normalized inbound media placeholders on supported channels

## Implemented Channels

- [x] Web
- [x] Telegram
- [x] Discord
- [x] Slack
- [x] Signal
- [x] WhatsApp Cloud API

## Missing or Partial vs OpenClaw

### Rich Channel Behavior

- [ ] Rich attachment/media handling across supported channels
- [ ] Broader edit support beyond Telegram
- [ ] Delete support where platforms allow it
- [ ] Channel-specific streaming or pseudo-streaming delivery
- [ ] Better group-routing modes and reply-tag semantics
- [ ] Better WhatsApp-specific behavior beyond normalized inbound media/webhook handling
- [ ] Broader platform-specific retry/backoff semantics

### Canvas Depth

- [ ] Structured Canvas document model beyond title/body text
- [ ] Session-linked Canvas workflows
- [ ] Incremental Canvas patches instead of full-document replace
- [ ] Multiple canvases or per-session canvases
- [ ] Safer agent-driven UI blocks/components
- [ ] Snapshot/export flows
- [ ] A2UI-style richer host capabilities

### Tool Depth

- [ ] Browser automation runtime
- [ ] Browser session/profile management
- [ ] Visual/browser snapshots
- [ ] Safer action model for clicks/forms/navigation
- [ ] Tool approvals for higher-risk tool families
- [ ] More first-party tools beyond session inspection
- [ ] Better tool tracing and operator visibility

### Operator / Install

- [ ] Docker support and documented container flow
- [ ] Easier channel setup flows
- [ ] Better channel/provider setup verification in `doctor`
- [ ] Better deployment docs and examples
- [ ] Service/install guidance beyond `systemd` output
- [ ] Config examples per supported channel

### Test Coverage and Test Quality

- [ ] More integration coverage across supported channels
- [x] Better end-to-end coverage for operator flows
- [ ] External-API contract fixtures for supported channels
- [x] More failure-path tests for provider failover and retries
- [x] Coverage for Canvas RPC/UI behavior
- [x] Coverage for onboarding/install helpers
- [ ] Regression-focused tests for delivery metadata and session rewrites

### Still Missing OpenClaw Breadth

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

## Explicitly Lower Priority Right Now

- Wider long-tail channel parity
- Companion node/app surfaces
- Voice
- Distro-specific installers

## Near-Term Priority Order

1. Increase test coverage and test quality.
2. Improve rich behavior on already-supported channels.
3. Deepen Canvas into a structured, session-aware host surface.
4. Add browser automation as the first higher-risk tool family.
5. Improve operator/install quality with Docker support and better setup docs.
