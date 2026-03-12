# Privacy Policy

**Last updated:** March 12, 2026

This privacy policy describes how FrankClaw ("the application", "we", "our") handles information when you interact with it through connected messaging platforms including WhatsApp, Telegram, Discord, Slack, Signal, and other supported channels.

## What FrankClaw Is

FrankClaw is a self-hosted, open-source AI assistant gateway. It is operated by the individual or organization that deployed it. This policy covers the default behavior of the software. The operator may have additional policies that apply.

## Information We Process

When you send a message to FrankClaw through a supported messaging channel, the following information is processed:

- **Message content:** The text, images, audio, or files you send.
- **Sender identifiers:** Your platform-specific user ID (e.g., WhatsApp phone number, Telegram user ID, Discord user ID) used to maintain conversation context.
- **Message metadata:** Timestamps, message IDs, and channel-specific metadata required for message delivery.

## How Information Is Used

Your information is used solely to:

1. **Process your messages** and generate AI-assisted responses via configured model providers (e.g., OpenAI, Anthropic, Ollama).
2. **Maintain conversation context** within your session so responses are relevant to your ongoing conversation.
3. **Deliver responses** back to you on the same messaging platform.

We do not use your data for advertising, profiling, analytics, or any purpose beyond providing the assistant service.

## Data Storage

- **Session transcripts** are stored locally on the server running FrankClaw. When configured, transcripts are encrypted at rest using ChaCha20-Poly1305 authenticated encryption.
- **Media files** (images, audio, documents) you send may be temporarily stored for processing and are kept in a local file store on the server.
- **No data is sold, shared with, or transferred to third parties** beyond the AI model providers necessary to generate responses.

## Third-Party AI Providers

To generate responses, your messages are forwarded to the AI model provider(s) configured by the operator (e.g., OpenAI, Anthropic, or a local Ollama instance). Each provider has its own privacy policy and data handling practices:

- OpenAI: https://openai.com/privacy
- Anthropic: https://www.anthropic.com/privacy
- Ollama (local): Data stays on the server; no external transmission.

The operator chooses which providers are used. When using local models (Ollama), your data never leaves the server.

## Data Retention

- Session data is retained on the server for as long as the session is active.
- The operator may configure session pruning to automatically remove old sessions.
- You may request deletion of your session data by contacting the operator.

## Security Measures

FrankClaw implements the following security measures:

- Optional encryption at rest for stored transcripts
- SSRF protection on all outbound requests
- Input sanitization and size limits on all user input
- Constant-time token comparison for authentication
- No shell command execution without explicit operator policy
- Optional malware scanning on file uploads

## Data Subject Rights

Since FrankClaw is self-hosted, data subject requests (access, deletion, correction) should be directed to the operator running the instance. The software provides operator commands to list, inspect, and delete sessions.

## Children's Privacy

FrankClaw is not directed at children under 13. The operator is responsible for ensuring compliance with applicable age restrictions.

## Changes to This Policy

This policy may be updated as the software evolves. The "last updated" date at the top reflects the most recent revision.

## Contact

For questions about this privacy policy or your data, contact the operator of the FrankClaw instance you are interacting with.

For questions about the FrankClaw software itself, visit: https://github.com/AkitaOnRails/frankclaw
