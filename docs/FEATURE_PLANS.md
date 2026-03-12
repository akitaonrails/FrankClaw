# FrankClaw Feature Plans

These are the major feature families still worth building after the current hardened core.
They are ordered for the current product direction, not for full OpenClaw breadth.

## Planning Rules

- Keep the runtime path as the single execution path.
- Prefer depth on supported surfaces over breadth on new surfaces.
- Do not widen trust boundaries before auth, audit, and storage rules exist for that feature.
- New behavior needs unit tests first and at least one integration path before it counts as done.
- If a feature starts recreating OpenClaw sprawl without clear value, shrink the scope.

## Recommended Order

1. Test coverage and test quality
2. Rich channel behavior
3. Canvas depth
4. Tools runtime with browser automation
5. Operator and install experience
6. Web control UI and WebChat refinements
7. Skills and plugin system refinements
8. Webhooks and broader gateway control plane refinements
9. Tailscale and remote-access ops
10. Companion nodes and apps
11. Voice
12. Additional channels only if the product scope changes again

## 1. Test Coverage and Test Quality

Goal:
Raise confidence in the current supported FrankClaw surface so new feature work does not regress the hardened core.

Scope:
- integration coverage for supported channels
- reusable channel and provider fixtures
- failure-path coverage for retries, failover, and auth boundaries
- Canvas and tool orchestration coverage
- operator/onboarding/install command coverage

Dependencies:
- stable fake providers and fake channels
- signed webhook and auth test helpers
- consistent session and delivery metadata fixtures

Phases:
1. Build reusable provider/channel fixtures.
2. Add gateway-path integration coverage for all supported channels.
3. Add failure-path tests for retries, failover, auth, and size limits.
4. Add CLI tests for onboarding, install, doctor, and status flows.
5. Add Canvas and tool orchestration integration coverage.

Security constraints:
- Coverage must include authorization and policy boundaries, not just happy paths.
- Prefer deterministic fixtures over flaky live API tests.
- Regression-prone state transitions should get explicit tests.

Acceptance:
- Supported surfaces have integration coverage for their primary flows.
- Known regression hotspots have dedicated tests.

## 2. Rich Channel Behavior

Goal:
Bring the supported channels closer to OpenClaw’s routing and delivery behavior without adding more channel count.

Scope:
- broader edit/delete support where safe
- richer reply-thread and reply-tag behavior
- channel-specific chunking and delivery semantics
- broader attachment/media normalization
- richer group-routing modes
- streaming or pseudo-streaming where safe
- richer WhatsApp behavior beyond the basic Cloud API text loop

Dependencies:
- delivery metadata persisted per reply
- stable outbound retry logic
- channel capability flags enforced by the gateway

Phases:
1. Persist the extra outbound context needed for edit/delete/retry/media follow-up.
2. Extend edit support beyond Telegram where platform semantics are stable.
3. Refine shared chunking policy for supported platforms.
4. Add richer group-routing config and tests for mention, reply-tag, and thread modes.
5. Add safe attachment/media placeholders and delivery metadata.
6. Add streaming only where the adapter can update in place safely.

Security constraints:
- Never edit, stream, or delete into a different conversation than the recorded origin.
- Treat delivery metadata as sensitive session state.
- Do not widen markup support until escaping is proven per platform.

Acceptance:
- Supported channels preserve enough metadata for retries and follow-up actions.
- Long outputs and reply routing behave predictably per platform.

## 3. Canvas Depth

Goal:
Turn the current local Canvas host from a text document surface into a structured, session-aware visual workspace without introducing arbitrary code execution.

Scope:
- typed Canvas document schema
- session-linked canvases
- partial updates and patches
- safe UI blocks instead of raw HTML
- export and snapshot support

Do not include:
- arbitrary JS execution
- raw eval APIs
- untrusted widgets

Dependencies:
- existing local console host
- gateway event model
- auth and audit hooks

Phases:
1. Define a typed Canvas document schema with safe block primitives.
2. Add session-linked canvases and IDs instead of a single global document.
3. Add patch/update RPCs and event streaming for incremental updates.
4. Add safe rendering blocks in the local host.
5. Add export/snapshot flows and tests.
6. Consider A2UI-style richer host semantics only after the above is stable.

Security constraints:
- Canvas payloads are data, not executable code.
- Rendering must treat content as untrusted unless explicitly typed and escaped.
- Canvas updates follow the same auth and audit boundaries as chat and tools.

Acceptance:
- The agent or operator can update a structured Canvas for a session without replacing the whole document.
- The Canvas host never evaluates arbitrary code.

## 4. Tools Runtime With Browser Automation

Goal:
Add a hardened browser automation surface as the first high-value higher-risk tool family.

Scope:
- browser session lifecycle
- navigation, click, type, extract, snapshot
- explicit per-agent tool policy
- operator visibility into active browser sessions

Do not include:
- arbitrary shell execution
- free-form desktop automation
- device/node actions

Dependencies:
- bounded tool orchestration already exists
- stronger audit logging
- stable tool policy model

Phases:
1. Define browser tool capabilities and policy declarations.
2. Build a dedicated browser runtime boundary separate from the gateway process.
3. Add safe primitives: open, navigate, snapshot, extract text, click by selector, type by selector.
4. Add isolated browser profiles and cleanup guarantees.
5. Add screenshots/snapshots plus operator visibility into active sessions.

Security constraints:
- Browser automation must not share the gateway process trust boundary.
- Profiles must be isolated and cleanup must be reliable.
- Tool inputs and outputs must be audited and attributable.
- Navigation, downloads, and uploads should be policy-bounded.

Acceptance:
- One agent can use browser automation through explicit policy with audited, isolated execution.

## 5. Operator and Install Experience

Goal:
Make the supported FrankClaw surface easy to set up and run without hiding critical security choices.

Scope:
- stronger onboarding flows
- easier supported-channel setup
- Docker support
- better docs and examples
- stronger `doctor`

Do not include:
- distro-specific installers
- convenience wrappers that silently weaken security defaults

Dependencies:
- stable supported channel/provider matrix
- stable config schema

Phases:
1. Expand `onboard` into profile-based secure config generation for supported setups.
2. Add Docker support with volume, env, and secret guidance.
3. Add supported channel/provider setup examples and docs.
4. Improve `doctor` to check env refs, file permissions, bind/auth posture, and known setup prerequisites.
5. Add deployment docs for local, Docker, and `systemd` paths.

Security constraints:
- Generated configs must keep secure defaults.
- Docker examples must not default to insecure binds or plaintext world-readable secrets.
- Operator guidance must clearly distinguish local-only, tailnet-only, and public deployments.

Acceptance:
- A user can set up a supported channel/provider combination with docs and generated config, without reading source code.

## 6. Web Control UI and WebChat Refinements

Goal:
Improve the local console and WebChat without weakening the gateway’s local-first posture.

Scope:
- better session inspection
- Canvas integration polish
- richer tool/operator views
- safer config inspection and limited editing

Dependencies:
- stable WS methods
- browser auth story already in place

Phases:
1. Improve session and transcript inspection UX.
2. Improve Canvas and tool visibility in the UI.
3. Add limited config editing only after validation and audit hooks are explicit.

Security constraints:
- Loopback remains the default bind mode.
- Browser-visible data must stay redacted.
- UI actions must follow auth-role checks.

Acceptance:
- The local browser UI is a credible operator surface for the supported product.

## 7. Skills and Plugin System Refinements

Goal:
Keep the extension model constrained while making workspace-local skills more usable and observable.

Scope:
- better manifest capabilities
- stronger validation and docs
- clearer operator visibility

Do not include:
- remote marketplace
- arbitrary code loading without isolation

Dependencies:
- tool policy model
- audit visibility

Phases:
1. Refine manifest capabilities and validation.
2. Improve local skill packaging and examples.
3. Add better runtime/operator visibility for loaded skills.

Security constraints:
- Extensions remain opt-in and fail closed on malformed manifests.
- No implicit network, filesystem, or exec privileges.

Acceptance:
- Workspace-local skills are easy to validate, understand, and audit.

## 8. Webhooks and Broader Gateway Control Plane Refinements

Goal:
Fill in more of the gateway control surface without allowing new inputs to bypass the runtime or policy model.

Scope:
- presence and typing indicators
- richer admin/session methods
- safer mutation APIs

Dependencies:
- current WS methods remain stable
- audit logging exists

Phases:
1. Add read-only control-plane methods first.
2. Add event-only presence and typing surfaces.
3. Add mutation methods only with explicit auth-role checks and audit logs.

Security constraints:
- No admin mutation path may bypass runtime policy.
- Presence events must not leak private channel metadata across clients.

Acceptance:
- More gateway control-plane surfaces exist without weakening auth or runtime policy.

## 9. Tailscale and Remote Access Ops

Goal:
Support remote access patterns similar to OpenClaw while keeping local-first defaults and refusing dangerous exposure modes.

Scope:
- Tailscale identity-header mode
- optional serve/funnel helpers
- remote gateway operator guidance

Dependencies:
- trusted proxy and Tailscale auth modes already exist
- remote operator surface is useful enough to justify exposure

Phases:
1. Add config validation for remote-access modes.
2. Add explicit operator commands for checking remote exposure state.
3. Support tailnet-only access before any public mode.
4. Add public exposure only with mandatory auth and explicit confirmation.

Security constraints:
- Loopback remains the default bind mode.
- Tailnet/public exposure must refuse startup without valid auth.
- Never silently change network exposure for the operator.

Acceptance:
- Operators can expose the gateway remotely in a bounded, auditable way.

## 10. Companion Nodes and Apps

Goal:
Add device-local execution surfaces only after the gateway and tool trust boundaries are mature.

Scope:
- node pairing
- node inventory and capability discovery
- device-local actions on paired nodes

Dependencies:
- pairing model for device trust
- tool runtime
- node RPC protocol and capability model

Phases:
1. Define node identity, pairing, and capability advertisement.
2. Add a minimal node RPC protocol.
3. Build a simple reference node before full apps.

Security constraints:
- Node trust is separate from DM pairing.
- Every node action requires capability checks.
- Node revocation must be durable and immediate.

Acceptance:
- A paired node can advertise capabilities and perform one bounded action family with audit trails.

## 11. Voice

Goal:
Add speech input and output without turning the gateway into an ambient surveillance surface by default.

Scope:
- speech-to-text
- text-to-speech
- explicit activation or push-to-talk first

Dependencies:
- node/app surfaces or a local client that can capture audio
- media pipeline for audio blobs
- session routing for voice-originated turns

Phases:
1. Add audio ingestion and transcription.
2. Add TTS output for explicit chat responses.
3. Add push-to-talk flow.

Security constraints:
- Voice capture must be explicit and user-driven first.
- Audio retention must be bounded and configurable.

Acceptance:
- A user can submit spoken input, get a transcript-backed assistant reply, and optionally receive TTS output.

## 12. Additional Channels

Goal:
Only revisit new channel breadth if the product direction changes again.

Scope:
- the remaining OpenClaw long-tail channels

Dependencies:
- richer behavior on supported channels should be solid first
- operator setup/docs burden must stay reasonable

Acceptance:
- New channels should only be added when they serve a clear product need.

## Cross-Cutting Work

- Transcript and reply metadata schema versioning
- Better test fixtures for mocked providers and mocked channels
- Audit log verification tests
- Performance and backpressure tests on inbound/outbound queues
- Clear docs about what is intentionally out of scope
