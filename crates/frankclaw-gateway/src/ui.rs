use axum::response::Html;

pub async fn index() -> Html<&'static str> {
    Html(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>FrankClaw Console</title>
  <script src="https://cdn.tailwindcss.com"></script>
  <script src="https://cdn.jsdelivr.net/npm/@hotwired/stimulus@3.2.2/dist/stimulus.umd.js"></script>
  <script src="https://cdn.jsdelivr.net/npm/marked@15.0.7/marked.min.js"></script>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=DM+Sans:ital,opsz,wght@0,9..40,400;0,9..40,500;0,9..40,600;0,9..40,700&family=Syne:wght@600;700;800&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
  <script>
    tailwind.config = {
      theme: {
        extend: {
          colors: {
            ink: '#1c2230',
            muted: '#6b7280',
            cream: { DEFAULT: '#f4efe5', light: '#fbf8f1' },
            accent: { DEFAULT: '#0e6b50', dark: '#165c4a', soft: 'rgba(14,107,80,0.10)' },
            warn: '#8d4d00',
            panel: { DEFAULT: 'rgba(255,255,255,0.82)', strong: 'rgba(255,255,255,0.94)' },
          },
          fontFamily: {
            display: ['"Syne"', 'sans-serif'],
            body: ['"DM Sans"', 'sans-serif'],
            mono: ['"JetBrains Mono"', 'monospace'],
          },
          boxShadow: {
            glass: '0 8px 32px rgba(33,33,52,0.08)',
            lifted: '0 22px 60px rgba(33,33,52,0.12)',
          },
        },
      },
    };
  </script>
  <style>
    body {
      background:
        radial-gradient(circle at 10% 0%, rgba(14,107,80,0.15), transparent 30%),
        radial-gradient(circle at 90% 0%, rgba(206,122,44,0.10), transparent 28%),
        linear-gradient(180deg, #fbf8f1 0%, #f4efe5 100%);
    }
    .tab-btn { transition: all 150ms ease; }
    .tab-btn.active {
      background: #0e6b50;
      color: white;
      box-shadow: 0 2px 8px rgba(14,107,80,0.25);
    }
    .tab-panel { display: none; }
    .tab-panel.active { display: flex; }
    .msg-enter {
      animation: msgSlide 200ms ease-out;
    }
    @keyframes msgSlide {
      from { opacity: 0; transform: translateY(8px); }
      to   { opacity: 1; transform: translateY(0); }
    }
    .feed-area::-webkit-scrollbar { width: 6px; }
    .feed-area::-webkit-scrollbar-track { background: transparent; }
    .feed-area::-webkit-scrollbar-thumb { background: rgba(28,34,48,0.15); border-radius: 3px; }
    .feed-area::-webkit-scrollbar-thumb:hover { background: rgba(28,34,48,0.25); }
    @keyframes pulse-dot {
      0%, 100% { opacity: 1; }
      50% { opacity: 0.4; }
    }
    .status-dot { animation: pulse-dot 2s ease-in-out infinite; }
    textarea:focus, input:focus, select:focus {
      outline: none;
      border-color: #0e6b50;
      box-shadow: 0 0 0 3px rgba(14,107,80,0.12);
    }
    pre { tab-size: 2; }
    /* Markdown inside bubbles */
    .md-rendered { line-height: 1.7; }
    .md-rendered p { margin-bottom: 0.5em; }
    .md-rendered p:last-child { margin-bottom: 0; }
    .md-rendered ul, .md-rendered ol { margin: 0.4em 0; padding-left: 1.4em; }
    .md-rendered li { margin-bottom: 0.15em; }
    .md-rendered pre { background: rgba(28,34,48,0.06); border-radius: 0.5rem; padding: 0.75rem 1rem; overflow-x: auto; margin: 0.5em 0; font-size: 0.8rem; }
    .md-rendered code { font-family: 'JetBrains Mono', monospace; font-size: 0.85em; }
    .md-rendered :not(pre) > code { background: rgba(28,34,48,0.07); padding: 0.15em 0.35em; border-radius: 0.25rem; }
    .md-rendered blockquote { border-left: 3px solid #0e6b50; padding-left: 0.75em; margin: 0.5em 0; color: #6b7280; }
    .md-rendered h1, .md-rendered h2, .md-rendered h3 { font-weight: 700; margin: 0.6em 0 0.3em; }
    .md-rendered h1 { font-size: 1.15em; }
    .md-rendered h2 { font-size: 1.05em; }
    .md-rendered h3 { font-size: 1em; }
    .md-rendered a { color: #0e6b50; text-decoration: underline; }
    .md-rendered hr { border: none; border-top: 1px solid rgba(28,34,48,0.12); margin: 0.75em 0; }
    .md-rendered table { border-collapse: collapse; margin: 0.5em 0; font-size: 0.85em; }
    .md-rendered th, .md-rendered td { border: 1px solid rgba(28,34,48,0.12); padding: 0.3em 0.6em; }
    .md-rendered th { background: rgba(28,34,48,0.04); font-weight: 600; }
  </style>
</head>
<body class="min-h-screen font-body text-ink antialiased" data-controller="tabs">

  <!-- ===== Header ===== -->
  <header class="sticky top-0 z-50 bg-panel-strong/90 backdrop-blur-xl border-b border-ink/8">
    <div class="max-w-7xl mx-auto px-4 sm:px-6 h-14 flex items-center justify-between gap-4">
      <div class="flex items-center gap-3">
        <h1 class="font-display font-bold text-lg tracking-tight select-none">FrankClaw</h1>
        <span class="text-xs text-muted font-mono hidden sm:inline">console</span>
      </div>

      <nav class="flex gap-1 bg-ink/[0.04] rounded-xl p-1">
        <button class="tab-btn active px-3 py-1.5 rounded-lg text-sm font-semibold"
                data-tabs-target="btn" data-tab="connect"
                data-action="click->tabs#switchTab">Connect</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold text-muted hover:text-ink"
                data-tabs-target="btn" data-tab="chat"
                data-action="click->tabs#switchTab">Chat</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold text-muted hover:text-ink"
                data-tabs-target="btn" data-tab="canvas"
                data-action="click->tabs#switchTab">Canvas</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold text-muted hover:text-ink"
                data-tabs-target="btn" data-tab="system"
                data-action="click->tabs#switchTab">System</button>
      </nav>

      <div class="flex items-center gap-2 text-sm font-medium" data-tabs-target="status">
        <span class="w-2 h-2 rounded-full bg-warn"></span>
        <span>Disconnected</span>
      </div>
    </div>
  </header>

  <!-- ===== Main Content ===== -->
  <main class="flex-1" style="height: calc(100vh - 3.5rem);">

    <!-- ── Connect Tab ── -->
    <section class="tab-panel active flex-col items-center justify-center p-6 sm:p-10 h-full overflow-y-auto"
             data-tabs-target="panel" data-tab="connect">
      <div class="w-full max-w-md mx-auto">
        <div class="bg-white/90 backdrop-blur-lg rounded-3xl shadow-lifted border border-ink/8 p-8">
          <div class="mb-8">
            <h2 class="font-display font-bold text-2xl mb-2">Connect to Gateway</h2>
            <p class="text-sm text-muted leading-relaxed">Authenticate with a token or password to open a WebSocket control channel.</p>
          </div>
          <div class="space-y-5">
            <label class="block">
              <span class="text-sm font-medium text-muted mb-1.5 block">Auth Token</span>
              <input id="auth-token" type="password" placeholder="Paste gateway token"
                     class="w-full border border-ink/12 rounded-xl px-4 py-3 bg-white text-sm font-body placeholder:text-ink/30">
            </label>
            <label class="block">
              <span class="text-sm font-medium text-muted mb-1.5 block">Password</span>
              <input id="auth-password" type="password" placeholder="Or use password auth"
                     class="w-full border border-ink/12 rounded-xl px-4 py-3 bg-white text-sm font-body placeholder:text-ink/30">
            </label>
            <button id="connect-btn"
                    class="w-full bg-gradient-to-br from-accent-dark to-accent text-white font-bold text-sm py-3.5 rounded-xl
                           hover:shadow-lg hover:shadow-accent/20 hover:-translate-y-0.5 active:translate-y-0
                           transition-all duration-150">
              Connect
            </button>
          </div>
          <p class="mt-5 text-xs text-muted leading-relaxed">
            For loopback with no auth configured, leave both fields empty and click Connect.
          </p>
        </div>
      </div>
    </section>

    <!-- ── Chat Tab ── -->
    <section class="tab-panel flex-col h-full" data-tabs-target="panel" data-tab="chat">

      <!-- Config bar -->
      <div class="shrink-0 px-4 sm:px-6 py-2.5 border-b border-ink/8 bg-white/40 backdrop-blur-sm flex items-center gap-3 flex-wrap">
        <input id="chat-agent" placeholder="Agent (default)"
               class="border border-ink/10 rounded-lg px-3 py-1.5 text-sm bg-white/80 w-32 font-body placeholder:text-ink/30">
        <input id="chat-session" placeholder="Session key"
               class="border border-ink/10 rounded-lg px-3 py-1.5 text-sm bg-white/80 w-48 font-mono text-xs placeholder:text-ink/30">
        <button id="reset-session-btn"
                class="text-xs font-semibold text-muted hover:text-warn px-3 py-1.5 rounded-lg border border-ink/10 bg-white/60
                       hover:border-warn/30 transition-colors">
          Reset Session
        </button>
        <button id="refresh-btn"
                class="text-xs font-semibold text-muted hover:text-accent px-3 py-1.5 rounded-lg border border-ink/10 bg-white/60
                       hover:border-accent/30 transition-colors ml-auto">
          Refresh
        </button>
      </div>

      <!-- Message feed -->
      <div class="flex-1 overflow-y-auto min-h-0">
        <div id="chat-feed"
             class="feed-area max-w-3xl mx-auto px-4 sm:px-6 py-4 space-y-3">
        </div>
      </div>

      <!-- Uploads preview -->
      <div id="chat-uploads" class="shrink-0 hidden px-4 sm:px-6 py-2 border-t border-ink/6 bg-accent-soft/30">
      </div>

      <!-- Input bar -->
      <div class="shrink-0 border-t border-ink/10 bg-white/60 backdrop-blur-sm px-4 sm:px-6 py-3">
        <div class="max-w-3xl mx-auto flex gap-3 items-end">
          <div class="flex-1 min-w-0">
            <textarea id="chat-message" rows="1" placeholder="Send a message... (Enter to send, Shift+Enter for newline)"
                      class="w-full border border-ink/12 rounded-2xl px-4 py-3 text-sm bg-white resize-none
                             font-body placeholder:text-ink/30 leading-relaxed
                             max-h-40 overflow-y-auto"></textarea>
            <div class="mt-1.5 flex items-center gap-3">
              <label class="inline-flex items-center gap-1.5 text-xs font-medium text-muted hover:text-accent cursor-pointer transition-colors">
                <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M18.375 12.739l-7.693 7.693a4.5 4.5 0 01-6.364-6.364l10.94-10.94A3 3 0 1119.5 7.372L8.552 18.32m.009-.01l-.01.01m5.699-9.941l-7.81 7.81a1.5 1.5 0 002.112 2.13" />
                </svg>
                Attach
                <input id="chat-attachments" type="file" multiple class="hidden">
              </label>
            </div>
          </div>
          <button id="send-btn"
                  class="shrink-0 bg-gradient-to-br from-accent-dark to-accent text-white font-bold text-sm
                         w-12 h-12 rounded-2xl flex items-center justify-center
                         hover:shadow-lg hover:shadow-accent/20 hover:-translate-y-0.5 active:translate-y-0
                         transition-all duration-150">
            <svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5">
              <path stroke-linecap="round" stroke-linejoin="round" d="M6 12L3.269 3.126A59.768 59.768 0 0121.485 12 59.77 59.77 0 013.27 20.876L5.999 12zm0 0h7.5" />
            </svg>
          </button>
        </div>
      </div>
    </section>

    <!-- ── Canvas Tab ── -->
    <section class="tab-panel flex-col h-full overflow-y-auto p-6" data-tabs-target="panel" data-tab="canvas">
      <div class="max-w-5xl mx-auto w-full grid md:grid-cols-2 gap-6">

        <!-- Editor -->
        <div class="bg-white/90 backdrop-blur-lg rounded-2xl shadow-glass border border-ink/8 p-6 space-y-4">
          <h2 class="font-display font-bold text-lg">Canvas Editor</h2>
          <div class="grid grid-cols-2 gap-3">
            <label class="block">
              <span class="text-xs font-medium text-muted mb-1 block">Title</span>
              <input id="canvas-title" placeholder="Untitled"
                     class="w-full border border-ink/10 rounded-lg px-3 py-2 text-sm bg-white font-body placeholder:text-ink/30">
            </label>
            <label class="block">
              <span class="text-xs font-medium text-muted mb-1 block">Canvas ID</span>
              <input id="canvas-id" placeholder="main"
                     class="w-full border border-ink/10 rounded-lg px-3 py-2 text-sm bg-white font-mono placeholder:text-ink/30">
            </label>
          </div>
          <label class="block">
            <span class="text-xs font-medium text-muted mb-1 block">Session Key</span>
            <input id="canvas-session" placeholder="Link to session"
                   class="w-full border border-ink/10 rounded-lg px-3 py-2 text-sm bg-white font-mono placeholder:text-ink/30">
          </label>
          <label class="block">
            <span class="text-xs font-medium text-muted mb-1 block">Body</span>
            <textarea id="canvas-body-input" rows="5" placeholder="Canvas body content"
                      class="w-full border border-ink/10 rounded-lg px-3 py-2 text-sm bg-white font-body resize-vertical placeholder:text-ink/30"></textarea>
          </label>
          <div class="flex gap-2">
            <button id="canvas-push-btn"
                    class="flex-1 bg-gradient-to-br from-accent-dark to-accent text-white font-semibold text-sm py-2.5 rounded-xl
                           hover:shadow-lg hover:shadow-accent/20 transition-all duration-150">
              Push Canvas
            </button>
            <button id="canvas-append-btn"
                    class="flex-1 bg-white border border-ink/12 text-ink font-semibold text-sm py-2.5 rounded-xl
                           hover:bg-cream transition-colors">
              Append Block
            </button>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <label class="block">
              <span class="text-xs font-medium text-muted mb-1 block">Block Kind</span>
              <select id="canvas-block-kind"
                      class="w-full border border-ink/10 rounded-lg px-3 py-2 text-sm bg-white font-body">
                <option value="markdown">Markdown</option>
                <option value="note">Note</option>
                <option value="code">Code</option>
                <option value="checklist">Checklist</option>
                <option value="status">Status</option>
                <option value="metric">Metric</option>
                <option value="action">Action</option>
              </select>
            </label>
            <label class="block">
              <span class="text-xs font-medium text-muted mb-1 block">Block Text</span>
              <input id="canvas-block-text" placeholder="Block content"
                     class="w-full border border-ink/10 rounded-lg px-3 py-2 text-sm bg-white font-body placeholder:text-ink/30">
            </label>
          </div>
          <div class="flex gap-2">
            <button id="canvas-export-md-btn"
                    class="flex-1 bg-white border border-ink/12 text-ink font-medium text-xs py-2 rounded-lg hover:bg-cream transition-colors">
              Export .md
            </button>
            <button id="canvas-export-json-btn"
                    class="flex-1 bg-white border border-ink/12 text-ink font-medium text-xs py-2 rounded-lg hover:bg-cream transition-colors">
              Export .json
            </button>
            <button id="canvas-clear-btn"
                    class="flex-1 bg-white border border-warn/20 text-warn font-medium text-xs py-2 rounded-lg hover:bg-warn/5 transition-colors">
              Clear
            </button>
          </div>
        </div>

        <!-- Preview -->
        <div class="space-y-4">
          <h2 class="font-display font-bold text-lg">Preview</h2>
          <div id="canvas-stage"
               class="min-h-[300px] bg-gradient-to-br from-accent-soft to-white/90 border border-ink/8 rounded-2xl p-6 space-y-3">
            <p class="text-sm text-muted">No canvas content yet.</p>
          </div>
        </div>
      </div>
    </section>

    <!-- ── System Tab ── -->
    <section class="tab-panel flex-col h-full overflow-y-auto p-6" data-tabs-target="panel" data-tab="system">
      <div class="max-w-5xl mx-auto w-full grid md:grid-cols-2 gap-6">

        <div class="bg-white/90 backdrop-blur-lg rounded-2xl shadow-glass border border-ink/8 p-6">
          <h2 class="font-display font-bold text-lg mb-4">Sessions</h2>
          <div id="sessions-list" class="space-y-2 max-h-72 overflow-y-auto feed-area">
            <p class="text-sm text-muted">No sessions yet.</p>
          </div>
        </div>

        <div class="bg-white/90 backdrop-blur-lg rounded-2xl shadow-glass border border-ink/8 p-6">
          <h2 class="font-display font-bold text-lg mb-4">Pairings</h2>
          <div id="pairings-list" class="space-y-2 max-h-72 overflow-y-auto feed-area">
            <p class="text-sm text-muted">No pending pairings.</p>
          </div>
        </div>

        <div class="bg-white/90 backdrop-blur-lg rounded-2xl shadow-glass border border-ink/8 p-6">
          <h2 class="font-display font-bold text-lg mb-4">Models</h2>
          <pre id="models-view"
               class="text-xs font-mono bg-cream/60 border border-ink/8 rounded-xl p-4 max-h-72 overflow-auto">[]</pre>
        </div>

        <div class="bg-white/90 backdrop-blur-lg rounded-2xl shadow-glass border border-ink/8 p-6">
          <h2 class="font-display font-bold text-lg mb-4">Channels</h2>
          <pre id="channels-view"
               class="text-xs font-mono bg-cream/60 border border-ink/8 rounded-xl p-4 max-h-72 overflow-auto">[]</pre>
        </div>
      </div>
    </section>

  </main>

  <script>
    // =========================================================================
    // Shared Gateway Module
    // =========================================================================
    const FC = {
      socket: null,
      nextId: 1,
      pending: new Map(),
      activeStreams: new Map(),
      selectedSession: "",
      currentCanvas: null,
      pendingAttachments: [],
      webClientId: (() => {
        try {
          const existing = sessionStorage.getItem("frankclaw-web-client-id");
          if (existing) return existing;
          const id = globalThis.crypto?.randomUUID?.()
            || ("web-" + Date.now() + "-" + Math.random().toString(16).slice(2));
          sessionStorage.setItem("frankclaw-web-client-id", id);
          return id;
        } catch (_) {
          return "web-" + Date.now() + "-" + Math.random().toString(16).slice(2);
        }
      })(),

      get connected() {
        return this.socket && this.socket.readyState === WebSocket.OPEN;
      },

      rpc(method, params = {}) {
        if (!this.connected) return Promise.reject(new Error("not connected"));
        const id = String(this.nextId++);
        this.socket.send(JSON.stringify({ type: "request", id, method, params }));
        return new Promise((resolve, reject) => {
          this.pending.set(id, { resolve, reject });
          setTimeout(() => {
            if (this.pending.has(id)) {
              this.pending.delete(id);
              reject(new Error("timeout: " + method));
            }
          }, 30000);
        });
      },

      buildWsUrl() {
        const url = new URL((location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/ws");
        this._appendAuth(url);
        return url.toString();
      },

      buildApiUrl(path) {
        const url = new URL(path, location.origin);
        this._appendAuth(url);
        return url;
      },

      _appendAuth(url) {
        const token = document.getElementById("auth-token").value.trim();
        const password = document.getElementById("auth-password").value.trim();
        if (token) url.searchParams.set("token", token);
        if (password) url.searchParams.set("password", password);
      },

      async apiFetch(path, options = {}) {
        const url = this.buildApiUrl(path);
        const headers = new Headers(options.headers || {});
        if (!headers.has("content-type") && typeof options.body === "string") {
          headers.set("content-type", "application/json");
        }
        const resp = await fetch(url, { headers, ...options });
        const body = await resp.json().catch(() => ({}));
        if (!resp.ok) throw new Error(body.error || "HTTP " + resp.status);
        return body;
      },

      downloadFile(filename, mimeType, content) {
        const blob = new Blob([content], { type: mimeType });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = filename;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
      },
    };

    // =========================================================================
    // Stimulus Controllers
    // =========================================================================
    const { Application, Controller } = Stimulus;
    const app = Application.start();

    // ── Tabs ──
    app.register("tabs", class extends Controller {
      static targets = ["btn", "panel", "status"];

      switchTab(e) {
        this.show(e.currentTarget.dataset.tab);
      }

      show(name) {
        this.btnTargets.forEach(btn => {
          const active = btn.dataset.tab === name;
          btn.classList.toggle("active", active);
          btn.classList.toggle("text-muted", !active);
        });
        this.panelTargets.forEach(panel => {
          const active = panel.dataset.tab === name;
          panel.classList.toggle("active", active);
        });
      }

      setStatus(text, connected) {
        const el = this.statusTarget;
        const dot = el.querySelector("span:first-child");
        const label = el.querySelector("span:last-child");
        if (dot) {
          dot.className = "w-2 h-2 rounded-full " + (connected ? "bg-accent status-dot" : "bg-warn");
        }
        if (label) label.textContent = text;
      }
    });

    // ── Connection ──
    app.register("connect", class extends Controller {
      connect() {
        document.getElementById("connect-btn").addEventListener("click", () => this.doConnect());
      }

      async doConnect() {
        const tabs = app.getControllerForElementAndIdentifier(
          document.querySelector('[data-controller="tabs"]'), "tabs"
        );
        tabs.setStatus("Connecting\u2026", false);

        // Persist credentials to localStorage
        try {
          const token = document.getElementById("auth-token").value.trim();
          const password = document.getElementById("auth-password").value.trim();
          if (token) localStorage.setItem("frankclaw-auth-token", token);
          else localStorage.removeItem("frankclaw-auth-token");
          if (password) localStorage.setItem("frankclaw-auth-password", password);
          else localStorage.removeItem("frankclaw-auth-password");
        } catch (_) {}

        if (FC.socket) FC.socket.close();

        const socket = new WebSocket(FC.buildWsUrl());
        FC.socket = socket;

        socket.addEventListener("open", async () => {
          tabs.setStatus("Connected", true);
          tabs.show("chat");
          try { await refreshAll(); } catch (e) { appendSystemBubble("error", e.message); }
        });

        socket.addEventListener("message", handleWsMessage);
        socket.addEventListener("close", () => tabs.setStatus("Disconnected", false));
        socket.addEventListener("error", () => tabs.setStatus("Connection error", false));
      }
    });

    // ── Chat ──
    app.register("chat", class extends Controller {
      connect() {
        const feed = document.getElementById("chat-feed");
        const msg = document.getElementById("chat-message");
        const sendBtn = document.getElementById("send-btn");
        const resetBtn = document.getElementById("reset-session-btn");
        const refreshBtn = document.getElementById("refresh-btn");
        const attachInput = document.getElementById("chat-attachments");

        // Enter to send
        msg.addEventListener("keydown", (e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            sendMessage();
          }
        });

        // Auto-resize textarea
        msg.addEventListener("input", () => {
          msg.style.height = "auto";
          msg.style.height = Math.min(msg.scrollHeight, 160) + "px";
        });

        sendBtn.addEventListener("click", sendMessage);
        resetBtn.addEventListener("click", resetSession);
        refreshBtn.addEventListener("click", () => refreshAll().catch(e => appendSystemBubble("error", e.message)));

        attachInput.addEventListener("change", async () => {
          const files = Array.from(attachInput.files || []);
          if (!files.length) return;
          try {
            await uploadAttachments(files);
            appendSystemBubble("system", "Uploaded " + files.length + " file" + (files.length > 1 ? "s" : ""));
          } catch (e) {
            appendSystemBubble("error", e.message);
          } finally {
            attachInput.value = "";
          }
        });
      }
    });

    // ── Canvas ──
    app.register("canvas", class extends Controller {
      connect() {
        const $ = id => document.getElementById(id);

        $("canvas-push-btn").addEventListener("click", async () => {
          try {
            const resp = await FC.rpc("canvas_set", {
              ...canvasParams(),
              title: $("canvas-title").value.trim(),
              body: $("canvas-body-input").value.trim(),
              blocks: FC.currentCanvas?.blocks || [],
            });
            renderCanvas(resp.canvas || null);
          } catch (e) { appendSystemBubble("error", e.message); }
        });

        $("canvas-append-btn").addEventListener("click", async () => {
          const text = $("canvas-block-text").value.trim();
          if (!text) return;
          const kind = $("canvas-block-kind").value;
          const block = { kind, text };
          if (kind === "status") block.meta = { level: "info" };
          else if (kind === "action") block.meta = { action: "prefill_chat", target: text };
          try {
            const resp = await FC.rpc("canvas_patch", { ...canvasParams(), append_blocks: [block] });
            $("canvas-block-text").value = "";
            renderCanvas(resp.canvas || null);
          } catch (e) { appendSystemBubble("error", e.message); }
        });

        $("canvas-export-md-btn").addEventListener("click", async () => {
          try {
            const resp = await FC.rpc("canvas_export", { ...canvasParams(), format: "markdown" });
            FC.downloadFile(resp.filename || "canvas.md", resp.mime_type || "text/markdown", resp.content || "");
          } catch (e) { appendSystemBubble("error", e.message); }
        });

        $("canvas-export-json-btn").addEventListener("click", async () => {
          try {
            const resp = await FC.rpc("canvas_export", { ...canvasParams(), format: "json" });
            FC.downloadFile(resp.filename || "canvas.json", resp.mime_type || "application/json", resp.content || "{}");
          } catch (e) { appendSystemBubble("error", e.message); }
        });

        $("canvas-clear-btn").addEventListener("click", async () => {
          try {
            await FC.rpc("canvas_clear", canvasParams());
            renderCanvas(null);
          } catch (e) { appendSystemBubble("error", e.message); }
        });
      }
    });

    // ── System ──
    app.register("system", class extends Controller {
      // Rendering is handled by refreshAll()
    });

    // =========================================================================
    // Chat Helpers
    // =========================================================================
    function scrollFeed() {
      const feed = document.getElementById("chat-feed");
      requestAnimationFrame(() => { feed.scrollTop = feed.scrollHeight; });
    }

    function appendBubble(role, content, attachments = []) {
      const feed = document.getElementById("chat-feed");
      const wrapper = document.createElement("div");
      wrapper.className = "msg-enter";

      if (role === "user") {
        wrapper.className += " flex justify-end";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl rounded-br-sm px-4 py-3 bg-accent text-white shadow-sm">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider opacity-60 mb-1">you</div>' +
            '<div class="bubble-content"></div>' +
          '</div>';
      } else if (role === "assistant") {
        wrapper.className += " flex justify-start";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl rounded-bl-sm px-4 py-3 bg-white border border-ink/8 shadow-sm">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider text-accent mb-1">assistant</div>' +
            '<div class="bubble-content"></div>' +
          '</div>';
      } else if (role === "error") {
        wrapper.className += " flex justify-start";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl px-4 py-3 bg-red-50 border border-red-200">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider text-red-500 mb-1">error</div>' +
            '<div class="bubble-content text-red-700"></div>' +
          '</div>';
      } else {
        // system
        wrapper.className += " flex justify-center";
        wrapper.innerHTML =
          '<div class="text-xs text-muted bg-ink/[0.04] px-4 py-1.5 rounded-full">' +
            '<span class="bubble-content"></span>' +
          '</div>';
      }

      const contentEl = wrapper.querySelector(".bubble-content");
      const useMarkdown = (role === "assistant");
      renderBubbleContent(contentEl, content, attachments, useMarkdown);
      wrapper._role = role;
      feed.appendChild(wrapper);
      scrollFeed();
      return wrapper;
    }

    function appendSystemBubble(role, content) {
      return appendBubble(role, content, []);
    }

    function renderBubbleContent(root, content, attachments, useMarkdown) {
      root.innerHTML = "";
      const text = String(content || "").trim();
      if (text) {
        const node = document.createElement("div");
        if (useMarkdown && typeof marked !== "undefined") {
          node.className = "md-rendered text-sm break-words";
          node.innerHTML = marked.parse(text);
          // Sanitize: open links in new tab, strip scripts
          node.querySelectorAll("a").forEach(a => { a.target = "_blank"; a.rel = "noreferrer"; });
          node.querySelectorAll("script,iframe,object,embed").forEach(el => el.remove());
        } else {
          node.className = "whitespace-pre-wrap text-sm leading-relaxed break-words";
          node.textContent = text;
        }
        root.appendChild(node);
      }
      if (attachments.length) {
        const list = document.createElement("div");
        list.className = "mt-2 space-y-2";
        for (const att of attachments) list.appendChild(buildAttachmentCard(att));
        root.appendChild(list);
      }
      if (!text && !attachments.length) {
        const empty = document.createElement("span");
        empty.className = "text-sm opacity-40 italic";
        empty.textContent = "empty";
        root.appendChild(empty);
      }
    }

    function buildAttachmentCard(att) {
      const card = document.createElement("div");
      card.className = "rounded-lg border border-ink/10 bg-cream/50 p-3 space-y-2";
      const mime = String(att?.mime_type || "application/octet-stream");
      const name = String(att?.filename || att?.media_id || "attachment");
      const url = att?.url || null;

      const label = document.createElement(url ? "a" : "div");
      label.className = "text-sm font-semibold " + (url ? "text-accent hover:underline" : "");
      if (url) { label.href = url; label.target = "_blank"; label.rel = "noreferrer"; }
      label.textContent = name;
      card.appendChild(label);

      const meta = document.createElement("div");
      meta.className = "text-[11px] text-muted font-mono";
      meta.textContent = mime;
      card.appendChild(meta);

      if (url && mime.startsWith("image/")) {
        const img = document.createElement("img");
        img.src = url; img.alt = name; img.loading = "lazy";
        img.className = "max-w-full rounded-lg border border-ink/8";
        card.appendChild(img);
      } else if (url && mime.startsWith("audio/")) {
        const audio = document.createElement("audio");
        audio.controls = true; audio.src = url;
        audio.className = "w-full";
        card.appendChild(audio);
      } else if (url && mime.startsWith("video/")) {
        const video = document.createElement("video");
        video.controls = true; video.src = url;
        video.className = "max-w-full rounded-lg";
        card.appendChild(video);
      }
      return card;
    }

    function transcriptAttachments(entry) {
      const a = entry?.metadata?.attachments;
      return Array.isArray(a) ? a : [];
    }

    // =========================================================================
    // Messaging
    // =========================================================================
    async function sendMessage() {
      const msg = document.getElementById("chat-message");
      const agent = document.getElementById("chat-agent");
      const session = document.getElementById("chat-session");
      const message = msg.value.trim();
      const attachments = [...FC.pendingAttachments];
      if (!message && !attachments.length) return;

      const preview = [message, attachments.map(a => a.filename || a.media_id).join(", ")].filter(Boolean).join("\n");
      appendBubble("user", preview || "(attachment)");

      try {
        if (attachments.length) {
          const body = {
            sender_id: FC.webClientId,
            sender_name: "FrankClaw Console",
            message: message || null,
            attachments,
          };
          if (agent.value.trim()) body.agent_id = agent.value.trim();
          if (session.value.trim()) body.session_key = session.value.trim();

          const resp = await FC.apiFetch("/api/web/inbound", { method: "POST", body: JSON.stringify(body) });
          if (resp.session_key) {
            FC.selectedSession = resp.session_key;
            session.value = resp.session_key;
          }
          const outbound = await drainWebOutbound();
          for (const item of outbound) appendBubble("assistant", item.text, item.attachments || []);
        } else {
          const params = { message };
          if (agent.value.trim()) params.agent_id = agent.value.trim();
          if (session.value.trim()) params.session_key = session.value.trim();
          const resp = await FC.rpc("chat_send", params);
          if (resp.session_key) {
            FC.selectedSession = resp.session_key;
            session.value = resp.session_key;
          }
        }
      } catch (e) {
        appendSystemBubble("error", e.message);
      }

      msg.value = "";
      msg.style.height = "auto";
      FC.pendingAttachments = [];
      renderPendingAttachments();
      refreshAll().catch(() => {});
    }

    async function resetSession() {
      const key = document.getElementById("chat-session").value.trim();
      if (!key) return;
      try {
        await FC.rpc("sessions_reset", { session_key: key });
        document.getElementById("chat-feed").innerHTML = "";
        await refreshAll();
      } catch (e) {
        appendSystemBubble("error", e.message);
      }
    }

    async function drainWebOutbound(maxAttempts = 12) {
      for (let i = 0; i < maxAttempts; i++) {
        const resp = await FC.apiFetch("/api/web/outbound?recipient_id=" + encodeURIComponent(FC.webClientId));
        if ((resp.messages || []).length) return resp.messages;
        await new Promise(r => setTimeout(r, 150));
      }
      return [];
    }

    // =========================================================================
    // Attachments
    // =========================================================================
    async function uploadAttachments(files) {
      for (const file of files) {
        const resp = await fetch(FC.buildApiUrl("/api/media/upload"), {
          method: "POST",
          headers: { "content-type": file.type || "application/octet-stream", "x-file-name": file.name },
          body: file,
        });
        const body = await resp.json().catch(() => ({}));
        if (!resp.ok) throw new Error(body.error || "HTTP " + resp.status);
        FC.pendingAttachments.push({
          media_id: body.media_id,
          mime_type: body.mime_type || file.type || "application/octet-stream",
          filename: body.filename || file.name,
          size_bytes: body.size_bytes || file.size || null,
        });
      }
      renderPendingAttachments();
    }

    function renderPendingAttachments() {
      const el = document.getElementById("chat-uploads");
      if (!FC.pendingAttachments.length) {
        el.classList.add("hidden");
        el.innerHTML = "";
        return;
      }
      el.classList.remove("hidden");
      el.innerHTML = FC.pendingAttachments.map(a =>
        '<span class="inline-flex items-center gap-1.5 text-xs font-medium text-accent bg-accent-soft rounded-full px-3 py-1 mr-2">' +
          '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M18.375 12.739l-7.693 7.693a4.5 4.5 0 01-6.364-6.364l10.94-10.94A3 3 0 1119.5 7.372L8.552 18.32"/></svg>' +
          (a.filename || a.media_id) +
        '</span>'
      ).join("");
    }

    // =========================================================================
    // Canvas Helpers
    // =========================================================================
    function canvasParams() {
      const p = {};
      const id = document.getElementById("canvas-id").value.trim();
      const sk = document.getElementById("canvas-session").value.trim();
      if (id) p.canvas_id = id;
      if (sk) p.session_key = sk;
      return p;
    }

    function renderCanvas(canvas) {
      FC.currentCanvas = canvas;
      const stage = document.getElementById("canvas-stage");

      if (!canvas) {
        stage.innerHTML = '<p class="text-sm text-muted">No canvas content yet.</p>';
        document.getElementById("canvas-id").value = "main";
        document.getElementById("canvas-title").value = "";
        document.getElementById("canvas-session").value = "";
        document.getElementById("canvas-body-input").value = "";
        document.getElementById("canvas-block-text").value = "";
        return;
      }

      document.getElementById("canvas-id").value = canvas.id || "";
      document.getElementById("canvas-title").value = canvas.title || "";
      document.getElementById("canvas-session").value = canvas.session_key || "";
      document.getElementById("canvas-body-input").value = canvas.body || "";

      stage.innerHTML = "";

      const title = document.createElement("h3");
      title.className = "font-display font-bold text-lg";
      title.textContent = canvas.title || "Untitled canvas";
      stage.appendChild(title);

      const meta = document.createElement("div");
      meta.className = "text-[11px] font-mono text-muted";
      meta.textContent = [canvas.id || "main", canvas.session_key || "no session", "rev " + (canvas.revision || 0), canvas.updated_at || "pending"].join(" \u00b7 ");
      stage.appendChild(meta);

      if (canvas.body) {
        const body = document.createElement("div");
        body.className = "text-sm whitespace-pre-wrap leading-relaxed mt-2";
        body.textContent = canvas.body;
        stage.appendChild(body);
      }

      for (const block of (canvas.blocks || [])) {
        stage.appendChild(renderCanvasBlock(block));
      }
    }

    function renderCanvasBlock(block) {
      const item = document.createElement("div");
      item.className = "rounded-xl border border-ink/10 bg-white/80 p-3 mt-2";
      const kind = block.kind || "block";
      const meta = block.meta || {};

      if (kind === "action") {
        item.innerHTML =
          '<div class="text-[10px] font-semibold uppercase tracking-wider text-muted mb-2">action \u00b7 ' + (meta.action || "noop") + '</div>';
        const btn = document.createElement("button");
        btn.className = "bg-white border border-ink/12 text-ink font-semibold text-sm py-2 px-4 rounded-lg hover:bg-cream transition-colors";
        btn.textContent = block.text || meta.label || "Run action";
        btn.addEventListener("click", () => runCanvasAction(meta).catch(e => appendSystemBubble("error", e.message)));
        item.appendChild(btn);
        return item;
      }

      const label = kind === "status" ? "status \u00b7 " + (meta.level || "info") : kind === "metric" ? "metric" : kind;
      item.innerHTML = '<div class="text-[10px] font-semibold uppercase tracking-wider text-muted mb-1">' + label + '</div>';

      const content = document.createElement("div");
      content.className = "text-sm whitespace-pre-wrap";
      if (kind === "metric") {
        const val = meta.value == null ? "" : String(meta.value);
        content.textContent = val && block.text ? block.text + ": " + val : (val || block.text || "");
      } else {
        content.textContent = block.text || "";
      }
      item.appendChild(content);
      return item;
    }

    async function runCanvasAction(meta) {
      const action = String(meta.action || "").trim();
      if (action === "open_url") {
        const raw = String(meta.target || meta.url || "").trim();
        const url = new URL(raw, location.origin);
        if (!["http:", "https:"].includes(url.protocol)) throw new Error("only http/https URLs allowed");
        window.open(url.toString(), "_blank", "noopener,noreferrer");
      } else if (action === "prefill_chat") {
        if (meta.agent_id) document.getElementById("chat-agent").value = String(meta.agent_id);
        if (meta.session_key) {
          document.getElementById("chat-session").value = String(meta.session_key);
          document.getElementById("canvas-session").value = String(meta.session_key);
        }
        const msg = document.getElementById("chat-message");
        msg.value = String(meta.target || meta.message || "");
        msg.focus();
        // Switch to chat tab
        document.querySelector('[data-tab="chat"]').click();
      } else if (action === "select_session") {
        const key = String(meta.session_key || meta.target || "").trim();
        if (!key) throw new Error("select_session requires session_key");
        await loadSession(key, meta.agent_id ? String(meta.agent_id) : null);
        document.querySelector('[data-tab="chat"]').click();
      } else {
        throw new Error("unsupported canvas action: " + (action || "noop"));
      }
    }

    // =========================================================================
    // Sessions / System
    // =========================================================================
    async function refreshAll() {
      const [sessions, pairings, models, channels, canvas] = await Promise.all([
        FC.rpc("sessions_list", { limit: 30 }),
        FC.apiFetch("/api/pairing/pending"),
        FC.rpc("models_list"),
        FC.rpc("channels_status"),
        FC.rpc("canvas_get", canvasParams()),
      ]);
      renderSessions(sessions.sessions || []);
      renderPairings(pairings.pending || []);
      document.getElementById("models-view").textContent = JSON.stringify(models.models || [], null, 2);
      document.getElementById("channels-view").textContent = JSON.stringify(channels.channels || [], null, 2);
      renderCanvas(canvas.canvas || null);
    }

    function renderSessions(items) {
      const el = document.getElementById("sessions-list");
      if (!items.length) {
        el.innerHTML = '<p class="text-sm text-muted">No sessions yet.</p>';
        return;
      }
      el.innerHTML = "";
      for (const item of items) {
        const btn = document.createElement("button");
        btn.className = "w-full text-left p-3 rounded-xl border border-ink/8 bg-white/60 hover:bg-white hover:shadow-sm transition-all text-sm";
        btn.innerHTML =
          '<div class="font-semibold">' + item.channel + " / " + item.account_id + '</div>' +
          '<div class="font-mono text-[11px] text-muted mt-0.5 truncate">' + item.key + '</div>';
        btn.addEventListener("click", async () => {
          await loadSession(item.key, item.agent_id || null);
          document.querySelector('[data-tab="chat"]').click();
        });
        el.appendChild(btn);
      }
    }

    async function loadSession(key, agentId) {
      FC.selectedSession = key;
      if (agentId) document.getElementById("chat-agent").value = agentId;
      document.getElementById("chat-session").value = key;
      document.getElementById("canvas-session").value = key;
      const history = await FC.rpc("chat_history", { session_key: key, limit: 50 });
      const feed = document.getElementById("chat-feed");
      feed.innerHTML = "";
      for (const entry of (history.entries || [])) {
        appendBubble(entry.role, entry.content, transcriptAttachments(entry));
      }
      const canvas = await FC.rpc("canvas_get", canvasParams());
      renderCanvas(canvas.canvas || null);
    }

    function renderPairings(items) {
      const el = document.getElementById("pairings-list");
      if (!items.length) {
        el.innerHTML = '<p class="text-sm text-muted">No pending pairings.</p>';
        return;
      }
      el.innerHTML = "";
      for (const item of items) {
        const btn = document.createElement("button");
        btn.className = "w-full text-left p-3 rounded-xl border border-accent/20 bg-accent-soft/30 hover:bg-accent-soft transition-all text-sm";
        btn.innerHTML =
          '<div class="font-semibold text-accent">' + item.channel + " / " + item.account_id + '</div>' +
          '<div class="font-mono text-[11px] text-muted mt-0.5">' + item.sender_id + ' \u00b7 ' + item.code + '</div>' +
          '<div class="text-[11px] text-accent font-medium mt-1">Click to approve</div>';
        btn.addEventListener("click", async () => {
          try {
            await FC.apiFetch("/api/pairing/approve", {
              method: "POST",
              body: JSON.stringify({ channel: item.channel, code: item.code, account: item.account_id }),
            });
            appendSystemBubble("system", "Approved pairing " + item.code);
            await refreshAll();
          } catch (e) {
            appendSystemBubble("error", e.message);
          }
        });
        el.appendChild(btn);
      }
    }

    // =========================================================================
    // WebSocket Message Handler
    // =========================================================================
    function handleWsMessage(event) {
      const frame = JSON.parse(event.data);

      if (frame.type === "response") {
        const p = FC.pending.get(String(frame.id));
        if (!p) return;
        FC.pending.delete(String(frame.id));
        if (frame.error) p.reject(new Error(frame.error.message || "request failed"));
        else p.resolve(frame.result || {});
        return;
      }

      if (frame.type !== "event") return;

      if (frame.event === "chat_delta") {
        const rid = String(frame.payload?.request_id || "");
        if (!rid || frame.payload?.kind !== "text") return;
        let bubble = FC.activeStreams.get(rid);
        if (!bubble) {
          bubble = appendBubble("assistant", "");
          FC.activeStreams.set(rid, bubble);
        }
        const root = bubble.querySelector(".bubble-content");
        const current = root.querySelector("div")?.textContent || "";
        renderBubbleContent(root, current + String(frame.payload?.delta || ""), [], false);
        scrollFeed();
        return;
      }

      if (frame.event === "chat_complete") {
        const rid = String(frame.payload?.request_id || "");
        if (rid && FC.activeStreams.has(rid)) {
          const bubble = FC.activeStreams.get(rid);
          renderBubbleContent(bubble.querySelector(".bubble-content"), frame.payload?.content || "", [], true);
          FC.activeStreams.delete(rid);
        } else if (frame.payload?.content) {
          appendBubble("assistant", frame.payload.content);
        }
        return;
      }

      if (frame.event === "chat_error") {
        const rid = String(frame.payload?.request_id || "");
        if (rid && FC.activeStreams.has(rid)) {
          FC.activeStreams.get(rid).remove();
          FC.activeStreams.delete(rid);
        }
        if (frame.payload?.message) appendSystemBubble("error", frame.payload.message);
        return;
      }

      if (frame.event === "canvas_updated") {
        if (frame.payload?.canvas) renderCanvas(frame.payload.canvas);
        else if ((frame.payload?.canvas_id || "main") === (document.getElementById("canvas-id").value.trim() || "main")) renderCanvas(null);
        return;
      }

      if (frame.event === "session_updated" && FC.selectedSession) {
        FC.rpc("chat_history", { session_key: FC.selectedSession, limit: 50 })
          .then(history => {
            document.getElementById("chat-feed").innerHTML = "";
            for (const entry of (history.entries || [])) {
              appendBubble(entry.role, entry.content, transcriptAttachments(entry));
            }
          })
          .catch(() => {});
      }
    }

    // =========================================================================
    // Init
    // =========================================================================

    // Restore saved credentials from localStorage
    try {
      const savedToken = localStorage.getItem("frankclaw-auth-token");
      const savedPassword = localStorage.getItem("frankclaw-auth-password");
      if (savedToken) document.getElementById("auth-token").value = savedToken;
      if (savedPassword) document.getElementById("auth-password").value = savedPassword;
    } catch (_) {}

    // Configure marked for safe rendering
    marked.setOptions({ breaks: true, gfm: true });

    renderPendingAttachments();
  </script>

  <!-- Stimulus controller declarations (connect to DOM) -->
  <div data-controller="connect" class="hidden"></div>
  <div data-controller="chat" class="hidden"></div>
  <div data-controller="canvas" class="hidden"></div>
  <div data-controller="system" class="hidden"></div>

</body>
</html>"##,
    )
}
