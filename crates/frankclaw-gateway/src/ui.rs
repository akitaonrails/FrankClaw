use axum::response::Html;

pub async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>FrankClaw Console</title>
  <style>
    :root {
      --bg: #f4efe5;
      --panel: rgba(255,255,255,0.82);
      --panel-strong: rgba(255,255,255,0.94);
      --ink: #1c2230;
      --muted: #6b7280;
      --line: rgba(28,34,48,0.12);
      --accent: #0e6b50;
      --accent-soft: rgba(14,107,80,0.12);
      --warn: #8d4d00;
      --shadow: 0 22px 60px rgba(33, 33, 52, 0.12);
      --radius: 22px;
      --mono: "SFMono-Regular", "Consolas", "Liberation Mono", monospace;
      --serif: "Iowan Old Style", "Palatino Linotype", "Book Antiqua", Palatino, Georgia, serif;
    }

    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: var(--serif);
      color: var(--ink);
      background:
        radial-gradient(circle at top left, rgba(14,107,80,0.18), transparent 28%),
        radial-gradient(circle at top right, rgba(206,122,44,0.14), transparent 26%),
        linear-gradient(180deg, #fbf8f1 0%, var(--bg) 100%);
      min-height: 100vh;
    }

    main {
      max-width: 1280px;
      margin: 0 auto;
      padding: 28px 20px 40px;
    }

    header {
      display: flex;
      justify-content: space-between;
      align-items: flex-start;
      gap: 16px;
      margin-bottom: 22px;
    }

    h1 {
      margin: 0;
      font-size: clamp(2.2rem, 5vw, 4rem);
      line-height: 0.92;
      letter-spacing: -0.05em;
      font-weight: 700;
    }

    header p {
      margin: 10px 0 0;
      max-width: 42rem;
      color: var(--muted);
      font-size: 1.02rem;
    }

    .status-pill {
      padding: 10px 14px;
      border-radius: 999px;
      background: var(--panel-strong);
      border: 1px solid var(--line);
      box-shadow: var(--shadow);
      font-size: 0.95rem;
    }

    .shell {
      display: grid;
      grid-template-columns: 1.2fr 0.8fr;
      gap: 18px;
    }

    .column {
      display: grid;
      gap: 18px;
    }

    .panel {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
      backdrop-filter: blur(18px);
      padding: 18px;
    }

    .panel h2 {
      margin: 0 0 12px;
      font-size: 1.15rem;
    }

    .grid {
      display: grid;
      gap: 12px;
    }

    .auth-row, .chat-row {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 10px;
    }

    .chat-row.single {
      grid-template-columns: 1fr;
    }

    label {
      display: grid;
      gap: 6px;
      font-size: 0.9rem;
      color: var(--muted);
    }

    input, textarea, button, select {
      width: 100%;
      border: 1px solid rgba(28,34,48,0.14);
      border-radius: 14px;
      padding: 12px 14px;
      font: inherit;
      background: rgba(255,255,255,0.92);
      color: var(--ink);
    }

    textarea {
      min-height: 128px;
      resize: vertical;
    }

    button {
      cursor: pointer;
      font-weight: 700;
      letter-spacing: 0.01em;
      background: linear-gradient(135deg, #165c4a 0%, var(--accent) 100%);
      color: #f8fbfa;
      border: none;
      transition: transform 120ms ease, box-shadow 120ms ease;
    }

    button:hover {
      transform: translateY(-1px);
      box-shadow: 0 12px 24px rgba(14,107,80,0.18);
    }

    button.secondary {
      background: var(--panel-strong);
      color: var(--ink);
      border: 1px solid var(--line);
      box-shadow: none;
    }

    .list {
      display: grid;
      gap: 8px;
      max-height: 260px;
      overflow: auto;
    }

    .list button {
      text-align: left;
      background: rgba(255,255,255,0.84);
      color: var(--ink);
      border: 1px solid var(--line);
      box-shadow: none;
      padding: 12px 12px;
    }

    .list button strong {
      display: block;
      font-size: 0.92rem;
    }

    .list button span {
      display: block;
      margin-top: 4px;
      font-size: 0.82rem;
      color: var(--muted);
      font-family: var(--mono);
    }

    .feed, pre {
      background: rgba(247,244,238,0.92);
      border: 1px solid var(--line);
      border-radius: 16px;
      padding: 14px;
      margin: 0;
      overflow: auto;
      font-family: var(--mono);
      font-size: 0.88rem;
    }

    .canvas-stage {
      min-height: 180px;
      background: linear-gradient(160deg, rgba(14,107,80,0.08), rgba(255,255,255,0.92));
      border: 1px solid var(--line);
      border-radius: 18px;
      padding: 18px;
      display: grid;
      gap: 10px;
    }

    .canvas-stage h3 {
      margin: 0;
      font-size: 1.2rem;
    }

    .canvas-meta {
      color: var(--muted);
      font-size: 0.82rem;
      font-family: var(--mono);
    }

    .canvas-body {
      white-space: pre-wrap;
      line-height: 1.5;
      font-size: 0.98rem;
    }

    .feed {
      min-height: 320px;
      display: grid;
      gap: 10px;
      align-content: start;
    }

    .bubble {
      padding: 12px;
      border-radius: 14px;
      border: 1px solid var(--line);
      background: rgba(255,255,255,0.88);
    }

    .bubble small {
      display: block;
      margin-bottom: 6px;
      color: var(--muted);
      font-size: 0.75rem;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }

    .muted {
      color: var(--muted);
    }

    .warning {
      color: var(--warn);
    }

    @media (max-width: 1024px) {
      .shell { grid-template-columns: 1fr; }
    }
  </style>
</head>
<body>
  <main>
    <header>
      <div>
        <h1>FrankClaw<br>Console</h1>
        <p>Local-first WebChat and operator surface for the hardened runtime. Connect with a token or password, chat over the gateway, inspect sessions, and watch model and channel health without leaving the loopback boundary.</p>
      </div>
      <div id="connection-status" class="status-pill">Disconnected</div>
    </header>

    <div class="shell">
      <div class="column">
        <section class="panel">
          <h2>Connect</h2>
          <div class="grid">
            <div class="auth-row">
              <label>Auth token
                <input id="auth-token" type="password" placeholder="Optional token">
              </label>
              <label>Password
                <input id="auth-password" type="password" placeholder="Optional password">
              </label>
            </div>
            <div class="auth-row">
              <button id="connect-btn">Connect</button>
              <button id="refresh-btn" class="secondary">Refresh Panels</button>
            </div>
            <div id="connect-help" class="muted">For loopback + no auth, leave both fields empty. Browser auth uses query parameters for the local WebSocket connection.</div>
          </div>
        </section>

        <section class="panel">
          <h2>Chat</h2>
          <div class="grid">
            <div class="chat-row">
              <label>Agent
                <input id="chat-agent" placeholder="main">
              </label>
              <label>Session key
                <input id="chat-session" placeholder="Optional explicit session">
              </label>
            </div>
            <div class="chat-row single">
              <label>Message
                <textarea id="chat-message" placeholder="Ask FrankClaw something"></textarea>
              </label>
            </div>
            <div class="chat-row">
              <button id="send-btn">Send</button>
              <button id="reset-session-btn" class="secondary">Reset Selected Session</button>
            </div>
            <div id="chat-feed" class="feed"></div>
          </div>
        </section>
      </div>

      <div class="column">
        <section class="panel">
          <h2>Sessions</h2>
          <div id="sessions-list" class="list"></div>
        </section>

        <section class="panel">
          <h2>Pairings</h2>
          <div id="pairings-list" class="list"></div>
        </section>

        <section class="panel">
          <h2>Models</h2>
          <pre id="models-view">[]</pre>
        </section>

        <section class="panel">
          <h2>Canvas</h2>
          <div class="grid">
            <label>Title
              <input id="canvas-title" placeholder="Optional canvas title">
            </label>
            <label>Session key
              <input id="canvas-session" placeholder="Optional linked session">
            </label>
            <label>Body
              <textarea id="canvas-body-input" placeholder="Write the canvas body shown in the local host"></textarea>
            </label>
            <div class="chat-row">
              <button id="canvas-push-btn">Push Canvas</button>
              <button id="canvas-clear-btn" class="secondary">Clear Canvas</button>
            </div>
            <div id="canvas-stage" class="canvas-stage">
              <div class="muted">No canvas content yet.</div>
            </div>
          </div>
        </section>

        <section class="panel">
          <h2>Channels</h2>
          <pre id="channels-view">[]</pre>
        </section>
      </div>
    </div>
  </main>

  <script>
    const state = {
      socket: null,
      nextId: 1,
      pending: new Map(),
      selectedSession: "",
    };

    const els = {
      status: document.getElementById("connection-status"),
      token: document.getElementById("auth-token"),
      password: document.getElementById("auth-password"),
      connectBtn: document.getElementById("connect-btn"),
      refreshBtn: document.getElementById("refresh-btn"),
      sendBtn: document.getElementById("send-btn"),
      resetBtn: document.getElementById("reset-session-btn"),
      feed: document.getElementById("chat-feed"),
      message: document.getElementById("chat-message"),
      agent: document.getElementById("chat-agent"),
      session: document.getElementById("chat-session"),
      sessions: document.getElementById("sessions-list"),
      pairings: document.getElementById("pairings-list"),
      models: document.getElementById("models-view"),
      channels: document.getElementById("channels-view"),
      canvasTitle: document.getElementById("canvas-title"),
      canvasSession: document.getElementById("canvas-session"),
      canvasBodyInput: document.getElementById("canvas-body-input"),
      canvasPushBtn: document.getElementById("canvas-push-btn"),
      canvasClearBtn: document.getElementById("canvas-clear-btn"),
      canvasStage: document.getElementById("canvas-stage"),
    };

    function setStatus(text, isConnected) {
      els.status.textContent = text;
      els.status.style.color = isConnected ? "var(--accent)" : "var(--warn)";
    }

    function appendBubble(label, content) {
      const div = document.createElement("div");
      div.className = "bubble";
      div.innerHTML = `<small>${label}</small><div></div>`;
      div.querySelector("div").textContent = content;
      els.feed.prepend(div);
    }

    function buildWsUrl() {
      const url = new URL((location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/ws");
      appendAuthQuery(url);
      return url.toString();
    }

    function appendAuthQuery(url) {
      if (els.token.value.trim()) url.searchParams.set("token", els.token.value.trim());
      if (els.password.value.trim()) url.searchParams.set("password", els.password.value.trim());
    }

    async function apiFetch(path, options = {}) {
      const url = new URL(path, location.origin);
      appendAuthQuery(url);
      const response = await fetch(url, {
        headers: { "content-type": "application/json", ...(options.headers || {}) },
        ...options,
      });
      const body = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(body.error || `HTTP ${response.status}`);
      }
      return body;
    }

    function rpc(method, params = {}) {
      if (!state.socket || state.socket.readyState !== WebSocket.OPEN) {
        return Promise.reject(new Error("websocket is not connected"));
      }
      const id = String(state.nextId++);
      state.socket.send(JSON.stringify({
        type: "request",
        id,
        method,
        params,
      }));
      return new Promise((resolve, reject) => {
        state.pending.set(id, { resolve, reject });
        setTimeout(() => {
          if (state.pending.has(id)) {
            state.pending.delete(id);
            reject(new Error(`timeout waiting for ${method}`));
          }
        }, 10000);
      });
    }

    async function refreshPanels() {
      const [sessions, pairings, models, channels, canvas] = await Promise.all([
        rpc("sessions_list", { limit: 30 }),
        apiFetch("/api/pairing/pending"),
        rpc("models_list"),
        rpc("channels_status"),
        rpc("canvas_get"),
      ]);
      renderSessions(sessions.sessions || []);
      renderPairings(pairings.pending || []);
      els.models.textContent = JSON.stringify(models.models || [], null, 2);
      els.channels.textContent = JSON.stringify(channels.channels || [], null, 2);
      renderCanvas(canvas.canvas || null);
    }

    function renderSessions(items) {
      els.sessions.innerHTML = "";
      if (!items.length) {
        els.sessions.innerHTML = `<div class="muted">No sessions yet.</div>`;
        return;
      }

      for (const item of items) {
        const button = document.createElement("button");
        button.type = "button";
        button.innerHTML = `<strong>${item.channel} / ${item.account_id}</strong><span>${item.key}</span>`;
        button.addEventListener("click", async () => {
          state.selectedSession = item.key;
          els.session.value = item.key;
          const history = await rpc("chat_history", { session_key: item.key, limit: 50 });
          els.feed.innerHTML = "";
          for (const entry of history.entries || []) {
            appendBubble(entry.role, entry.content);
          }
        });
        els.sessions.appendChild(button);
      }
    }

    function renderPairings(items) {
      els.pairings.innerHTML = "";
      if (!items.length) {
        els.pairings.innerHTML = `<div class="muted">No pending pairings.</div>`;
        return;
      }

      for (const item of items) {
        const button = document.createElement("button");
        button.type = "button";
        button.innerHTML = `<strong>${item.channel} / ${item.account_id}</strong><span>${item.sender_id} · ${item.code}</span>`;
        button.addEventListener("click", async () => {
          await apiFetch("/api/pairing/approve", {
            method: "POST",
            body: JSON.stringify({
              channel: item.channel,
              code: item.code,
              account: item.account_id,
            }),
          });
          appendBubble("system", `Approved pairing ${item.code}`);
          await refreshPanels();
        });
        els.pairings.appendChild(button);
      }
    }

    function renderCanvas(canvas) {
      if (!canvas) {
        els.canvasStage.innerHTML = `<div class="muted">No canvas content yet.</div>`;
        els.canvasTitle.value = "";
        els.canvasSession.value = "";
        els.canvasBodyInput.value = "";
        return;
      }

      els.canvasTitle.value = canvas.title || "";
      els.canvasSession.value = canvas.session_key || "";
      els.canvasBodyInput.value = canvas.body || "";

      const title = document.createElement("h3");
      title.textContent = canvas.title || "Untitled canvas";

      const meta = document.createElement("div");
      meta.className = "canvas-meta";
      meta.textContent = [canvas.session_key || "no session", canvas.updated_at || "pending"].join(" · ");

      const body = document.createElement("div");
      body.className = "canvas-body";
      body.textContent = canvas.body || "";

      els.canvasStage.innerHTML = "";
      els.canvasStage.append(title, meta, body);
    }

    function handleMessage(event) {
      const frame = JSON.parse(event.data);
      if (frame.type === "response") {
        const pending = state.pending.get(String(frame.id));
        if (!pending) return;
        state.pending.delete(String(frame.id));
        if (frame.error) {
          pending.reject(new Error(frame.error.message || "request failed"));
        } else {
          pending.resolve(frame.result || {});
        }
        return;
      }

      if (frame.type === "event" && frame.event === "chat_complete") {
        if (frame.payload?.content) {
          appendBubble("assistant", frame.payload.content);
        }
      }
      if (frame.type === "event" && frame.event === "canvas_updated") {
        renderCanvas(frame.payload?.canvas || null);
      }
      if (frame.type === "event" && frame.event === "session_updated" && state.selectedSession) {
        rpc("chat_history", { session_key: state.selectedSession, limit: 50 })
          .then((history) => {
            els.feed.innerHTML = "";
            for (const entry of history.entries || []) {
              appendBubble(entry.role, entry.content);
            }
          })
          .catch(() => {});
      }
    }

    async function connect() {
      if (state.socket) {
        state.socket.close();
      }

      const socket = new WebSocket(buildWsUrl());
      state.socket = socket;
      setStatus("Connecting", false);

      socket.addEventListener("open", async () => {
        setStatus("Connected", true);
        try {
          await refreshPanels();
        } catch (error) {
          appendBubble("error", error.message);
        }
      });

      socket.addEventListener("message", handleMessage);
      socket.addEventListener("close", () => setStatus("Disconnected", false));
      socket.addEventListener("error", () => setStatus("Connection error", false));
    }

    els.connectBtn.addEventListener("click", () => connect().catch((error) => appendBubble("error", error.message)));
    els.refreshBtn.addEventListener("click", () => refreshPanels().catch((error) => appendBubble("error", error.message)));
    els.sendBtn.addEventListener("click", async () => {
      const message = els.message.value.trim();
      if (!message) return;
      appendBubble("user", message);
      const params = { message };
      if (els.agent.value.trim()) params.agent_id = els.agent.value.trim();
      if (els.session.value.trim()) params.session_key = els.session.value.trim();
      const response = await rpc("chat_send", params);
      if (response.session_key) {
        state.selectedSession = response.session_key;
        els.session.value = response.session_key;
      }
      if (response.content) {
        appendBubble("assistant", response.content);
      }
      els.message.value = "";
      await refreshPanels();
    });

    els.resetBtn.addEventListener("click", async () => {
      const sessionKey = els.session.value.trim();
      if (!sessionKey) return;
      await rpc("sessions_reset", { session_key: sessionKey });
      els.feed.innerHTML = "";
      await refreshPanels();
    });

    els.canvasPushBtn.addEventListener("click", async () => {
      const params = {
        title: els.canvasTitle.value.trim(),
        body: els.canvasBodyInput.value.trim(),
      };
      if (els.canvasSession.value.trim()) {
        params.session_key = els.canvasSession.value.trim();
      }
      const response = await rpc("canvas_set", params);
      renderCanvas(response.canvas || null);
    });

    els.canvasClearBtn.addEventListener("click", async () => {
      await rpc("canvas_clear");
      renderCanvas(null);
    });
  </script>
</body>
</html>"#,
    )
}
