# FrankClaw Setup Guide

This file is the practical setup companion to `README.md`.
It focuses on supported surfaces only: current channels, model providers, and the Chromium browser runtime.

## Baseline

Start from a secure profile:

```bash
frankclaw onboard --channel web
```

Then validate before first launch:

```bash
frankclaw check
frankclaw doctor
frankclaw status
```

## Model Providers

### OpenAI

```bash
export OPENAI_API_KEY="sk-..."
```

```json
{
  "models": {
    "providers": [
      {
        "id": "openai",
        "api": "openai",
        "api_key_ref": "OPENAI_API_KEY",
        "models": ["gpt-4o-mini"]
      }
    ],
    "default_model": "gpt-4o-mini"
  }
}
```

### Anthropic

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

```json
{
  "models": {
    "providers": [
      {
        "id": "anthropic",
        "api": "anthropic",
        "api_key_ref": "ANTHROPIC_API_KEY",
        "models": ["claude-3-5-sonnet-latest"]
      }
    ]
  }
}
```

### Ollama

```json
{
  "models": {
    "providers": [
      {
        "id": "ollama",
        "api": "ollama",
        "base_url": "http://127.0.0.1:11434",
        "models": ["llama3.1"]
      }
    ],
    "default_model": "llama3.1"
  }
}
```

## Channel Accounts

Each supported channel still lives under `channels.<name>.accounts[0]`.

### Web

```json
{
  "channels": {
    "web": {
      "enabled": true,
      "accounts": [],
      "extra": {
        "dm_policy": "open"
      }
    }
  }
}
```

### Telegram

```bash
export TELEGRAM_BOT_TOKEN="123456:telegram-bot-token"
```

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "accounts": [
        { "bot_token_env": "TELEGRAM_BOT_TOKEN" }
      ]
    }
  }
}
```

### Discord

```bash
export DISCORD_BOT_TOKEN="discord-bot-token"
```

```json
{
  "channels": {
    "discord": {
      "enabled": true,
      "accounts": [
        { "bot_token_env": "DISCORD_BOT_TOKEN" }
      ],
      "extra": {
        "require_mention_for_groups": true
      }
    }
  }
}
```

### Slack

```bash
export SLACK_APP_TOKEN="xapp-..."
export SLACK_BOT_TOKEN="xoxb-..."
```

```json
{
  "channels": {
    "slack": {
      "enabled": true,
      "accounts": [
        {
          "app_token_env": "SLACK_APP_TOKEN",
          "bot_token_env": "SLACK_BOT_TOKEN"
        }
      ],
      "extra": {
        "require_mention_for_groups": true
      }
    }
  }
}
```

### Signal

```bash
export SIGNAL_BASE_URL="http://127.0.0.1:8080"
export SIGNAL_ACCOUNT="+15551234567"
```

```json
{
  "channels": {
    "signal": {
      "enabled": true,
      "accounts": [
        {
          "base_url_env": "SIGNAL_BASE_URL",
          "account_env": "SIGNAL_ACCOUNT"
        }
      ]
    }
  }
}
```

### WhatsApp Cloud API

```bash
export WHATSAPP_ACCESS_TOKEN="EA..."
export WHATSAPP_PHONE_NUMBER_ID="1234567890"
export WHATSAPP_VERIFY_TOKEN="your-webhook-verify-token"
export WHATSAPP_APP_SECRET="meta-app-secret"
```

```json
{
  "channels": {
    "whatsapp": {
      "enabled": true,
      "accounts": [
        {
          "access_token_env": "WHATSAPP_ACCESS_TOKEN",
          "phone_number_id_env": "WHATSAPP_PHONE_NUMBER_ID",
          "verify_token_env": "WHATSAPP_VERIFY_TOKEN",
          "app_secret_env": "WHATSAPP_APP_SECRET"
        }
      ]
    }
  }
}
```

## Browser Runtime

Browser tools need a DevTools endpoint. Keep it loopback-only.

### Docker Compose

```bash
docker compose up -d chromium
export FRANKCLAW_BROWSER_DEVTOOLS_URL="http://127.0.0.1:9222/"
```

### Local Chromium

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

### Agent Tool Allowlist

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
          "browser.type"
        ]
      }
    }
  }
}
```

## Recommended Bring-Up Order

1. Start one model provider.
2. Start one channel.
3. Run `frankclaw doctor`.
4. Start the gateway.
5. Verify the core path with `frankclaw message send`.
6. If browser tools are enabled, verify Chromium with `frankclaw tools list` and:

```bash
frankclaw tools invoke --tool browser.open --session default:web:control --args '{"url":"https://example.com"}'
frankclaw tools invoke --tool browser.type --session default:web:control --args '{"selector":"input","text":"frankclaw"}'
frankclaw tools invoke --tool browser.click --session default:web:control --args '{"selector":"button"}'
```

## Common Checks

- `frankclaw doctor` should not report missing env vars.
- `frankclaw status` should show the provider as healthy.
- Non-loopback binds should only be used with real auth.
- Browser DevTools should stay on `127.0.0.1`.
