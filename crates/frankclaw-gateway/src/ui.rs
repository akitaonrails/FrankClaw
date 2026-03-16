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
  <!-- FOUC prevention: set theme before first paint -->
  <script>
    (function(){
      var saved = 'system';
      try { saved = localStorage.getItem('frankclaw-theme') || 'system'; } catch(_){}
      var resolved = saved;
      if (saved === 'system') {
        resolved = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
      }
      document.documentElement.setAttribute('data-theme', resolved);
      document.documentElement._fcThemePref = saved;
    })();
  </script>
  <script>
    tailwind.config = {
      theme: {
        extend: {
          colors: {
            ink: 'var(--color-ink)',
            muted: 'var(--color-muted)',
            cream: { DEFAULT: 'var(--color-cream)', light: 'var(--color-cream-light)' },
            accent: { DEFAULT: 'var(--color-accent)', dark: 'var(--color-accent-dark)', soft: 'var(--color-accent-soft)' },
            warn: 'var(--color-warn)',
            panel: { DEFAULT: 'var(--color-panel)', strong: 'var(--color-panel-strong)' },
            surface: 'var(--color-surface)',
            danger: 'var(--color-danger)',
          },
          fontFamily: {
            display: ['"Syne"', 'sans-serif'],
            body: ['"DM Sans"', 'sans-serif'],
            mono: ['"JetBrains Mono"', 'monospace'],
          },
          boxShadow: {
            glass: 'var(--shadow-glass)',
            lifted: 'var(--shadow-lifted)',
          },
        },
      },
    };
  </script>
  <style>
    /* ── Theme CSS custom properties ── */
    [data-theme="light"] {
      --color-ink: #1c2230;
      --color-muted: #6b7280;
      --color-cream: #f4efe5;
      --color-cream-light: #fbf8f1;
      --color-accent: #0e6b50;
      --color-accent-dark: #165c4a;
      --color-accent-soft: rgba(14,107,80,0.10);
      --color-warn: #8d4d00;
      --color-panel: rgba(255,255,255,0.82);
      --color-panel-strong: rgba(255,255,255,0.94);
      --color-surface: #ffffff;
      --color-danger: #dc2626;
      --color-border: rgba(28,34,48,0.08);
      --color-border-strong: rgba(28,34,48,0.12);
      --color-code-bg: rgba(28,34,48,0.06);
      --color-code-inline: rgba(28,34,48,0.07);
      --shadow-glass: 0 8px 32px rgba(33,33,52,0.08);
      --shadow-lifted: 0 22px 60px rgba(33,33,52,0.12);
      --bg-body: radial-gradient(circle at 10% 0%, rgba(14,107,80,0.15), transparent 30%),
                 radial-gradient(circle at 90% 0%, rgba(206,122,44,0.10), transparent 28%),
                 linear-gradient(180deg, #fbf8f1 0%, #f4efe5 100%);
      --scrollbar-thumb: rgba(28,34,48,0.15);
      --scrollbar-thumb-hover: rgba(28,34,48,0.25);
    }

    [data-theme="dark"] {
      --color-ink: #e4e7ec;
      --color-muted: #9ca3af;
      --color-cream: #1a1d24;
      --color-cream-light: #22262e;
      --color-accent: #34d399;
      --color-accent-dark: #2ab882;
      --color-accent-soft: rgba(52,211,153,0.12);
      --color-warn: #f59e0b;
      --color-panel: rgba(30,33,40,0.85);
      --color-panel-strong: rgba(26,29,36,0.95);
      --color-surface: #252830;
      --color-danger: #ef4444;
      --color-border: rgba(255,255,255,0.08);
      --color-border-strong: rgba(255,255,255,0.12);
      --color-code-bg: rgba(255,255,255,0.06);
      --color-code-inline: rgba(255,255,255,0.08);
      --shadow-glass: 0 8px 32px rgba(0,0,0,0.25);
      --shadow-lifted: 0 22px 60px rgba(0,0,0,0.35);
      --bg-body: radial-gradient(circle at 10% 0%, rgba(52,211,153,0.08), transparent 30%),
                 radial-gradient(circle at 90% 0%, rgba(245,158,11,0.06), transparent 28%),
                 linear-gradient(180deg, #1a1d24 0%, #141720 100%);
      --scrollbar-thumb: rgba(255,255,255,0.15);
      --scrollbar-thumb-hover: rgba(255,255,255,0.25);
    }

    body {
      background: var(--bg-body);
      color: var(--color-ink);
    }
    .tab-btn { transition: all 150ms ease; }
    .tab-btn.active {
      background: var(--color-accent);
      color: #ffffff;
      box-shadow: 0 2px 8px rgba(14,107,80,0.25);
    }
    [data-theme="dark"] .tab-btn.active {
      color: #111827;
      box-shadow: 0 2px 8px rgba(52,211,153,0.25);
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
    .feed-area::-webkit-scrollbar-thumb { background: var(--scrollbar-thumb); border-radius: 3px; }
    .feed-area::-webkit-scrollbar-thumb:hover { background: var(--scrollbar-thumb-hover); }
    @keyframes pulse-dot {
      0%, 100% { opacity: 1; }
      50% { opacity: 0.4; }
    }
    .status-dot { animation: pulse-dot 2s ease-in-out infinite; }
    textarea:focus, input:focus, select:focus {
      outline: none;
      border-color: var(--color-accent);
      box-shadow: 0 0 0 3px var(--color-accent-soft);
    }
    pre { tab-size: 2; }
    /* Markdown inside bubbles */
    .md-rendered { line-height: 1.7; }
    .md-rendered p { margin-bottom: 0.5em; }
    .md-rendered p:last-child { margin-bottom: 0; }
    .md-rendered ul, .md-rendered ol { margin: 0.4em 0; padding-left: 1.4em; }
    .md-rendered li { margin-bottom: 0.15em; }
    .md-rendered pre { background: var(--color-code-bg); border-radius: 0.5rem; padding: 0.75rem 1rem; overflow-x: auto; margin: 0.5em 0; font-size: 0.8rem; }
    .md-rendered code { font-family: 'JetBrains Mono', monospace; font-size: 0.85em; }
    .md-rendered :not(pre) > code { background: var(--color-code-inline); padding: 0.15em 0.35em; border-radius: 0.25rem; }
    .md-rendered blockquote { border-left: 3px solid var(--color-accent); padding-left: 0.75em; margin: 0.5em 0; color: var(--color-muted); }
    .md-rendered h1, .md-rendered h2, .md-rendered h3 { font-weight: 700; margin: 0.6em 0 0.3em; }
    .md-rendered h1 { font-size: 1.15em; }
    .md-rendered h2 { font-size: 1.05em; }
    .md-rendered h3 { font-size: 1em; }
    .md-rendered a { color: var(--color-accent); text-decoration: underline; }
    .md-rendered hr { border: none; border-top: 1px solid var(--color-border-strong); margin: 0.75em 0; }
    .md-rendered table { border-collapse: collapse; margin: 0.5em 0; font-size: 0.85em; }
    .md-rendered th, .md-rendered td { border: 1px solid var(--color-border-strong); padding: 0.3em 0.6em; }
    .md-rendered th { background: var(--color-code-bg); font-weight: 600; }

    /* ── Focus mode ── */
    body.focus-mode header, body.focus-mode .fc-header-chrome { display: none !important; }
    body.focus-mode main { height: 100vh !important; }
    body.focus-mode .tab-panel.active { border-radius: 0; }
    #focus-exit { display: none; }
    body.focus-mode #focus-exit { display: flex; }

    /* ── Tool sidebar ── */
    #tool-sidebar { width: 0; transition: width 200ms ease; overflow: hidden; }
    #tool-sidebar.open { width: min(50vw, 600px); }
    #tool-sidebar .sidebar-handle { cursor: col-resize; width: 4px; background: var(--color-border-strong); }
    #tool-sidebar .sidebar-handle:hover { background: var(--color-accent); }
    main.sidebar-open { margin-right: min(50vw, 600px); transition: margin-right 200ms ease; }

    /* ── Tab bar scroll for mobile ── */
    .tab-nav-scroll { overflow-x: auto; -webkit-overflow-scrolling: touch; scrollbar-width: none; }
    .tab-nav-scroll::-webkit-scrollbar { display: none; }
  </style>
</head>
<body class="min-h-screen font-body antialiased" data-controller="tabs">

  <!-- ===== Header ===== -->
  <header class="sticky top-0 z-50 backdrop-blur-xl" style="background: var(--color-panel-strong); border-bottom: 1px solid var(--color-border);">
    <div class="max-w-7xl mx-auto px-4 sm:px-6 h-14 flex items-center justify-between gap-4">
      <div class="flex items-center gap-3 fc-header-chrome">
        <h1 class="font-display font-bold text-lg tracking-tight select-none">FrankClaw</h1>
        <span class="text-xs font-mono hidden sm:inline" style="color: var(--color-muted);">console</span>
      </div>

      <nav class="tab-nav-scroll flex gap-1 rounded-xl p-1" style="background: var(--color-border);">
        <button class="tab-btn active px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap"
                data-tabs-target="btn" data-tab="connect"
                data-action="click->tabs#switchTab">Connect</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="chat"
                data-action="click->tabs#switchTab">Chat</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="canvas"
                data-action="click->tabs#switchTab">Canvas</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="system"
                data-action="click->tabs#switchTab">System</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="usage"
                data-action="click->tabs#switchTab">Usage</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="agents"
                data-action="click->tabs#switchTab">Agents</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="cron"
                data-action="click->tabs#switchTab">Cron</button>
        <button class="tab-btn px-3 py-1.5 rounded-lg text-sm font-semibold whitespace-nowrap" style="color: var(--color-muted);"
                data-tabs-target="btn" data-tab="logs"
                data-action="click->tabs#switchTab">Logs</button>
      </nav>

      <div class="flex items-center gap-3 fc-header-chrome">
        <!-- Theme toggle -->
        <button id="theme-toggle" title="Toggle theme" class="p-1.5 rounded-lg transition-colors" style="color: var(--color-muted);"
                onmouseenter="this.style.color='var(--color-ink)'" onmouseleave="this.style.color='var(--color-muted)'">
          <svg id="theme-icon-light" class="w-4 h-4 hidden" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="5"/><path d="M12 1v2m0 18v2M4.22 4.22l1.42 1.42m12.72 12.72l1.42 1.42M1 12h2m18 0h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/>
          </svg>
          <svg id="theme-icon-dark" class="w-4 h-4 hidden" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z"/>
          </svg>
          <svg id="theme-icon-system" class="w-4 h-4 hidden" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8m-4-4v4"/>
          </svg>
        </button>
        <!-- Focus toggle -->
        <button id="focus-toggle" title="Focus mode" class="p-1.5 rounded-lg transition-colors" style="color: var(--color-muted);"
                onmouseenter="this.style.color='var(--color-ink)'" onmouseleave="this.style.color='var(--color-muted)'">
          <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path d="M8 3H5a2 2 0 00-2 2v3m18 0V5a2 2 0 00-2-2h-3m0 18h3a2 2 0 002-2v-3M3 16v3a2 2 0 002 2h3"/>
          </svg>
        </button>
        <!-- Status -->
        <div class="flex items-center gap-2 text-sm font-medium" data-tabs-target="status">
          <span class="w-2 h-2 rounded-full" style="background: var(--color-warn);"></span>
          <span>Disconnected</span>
        </div>
      </div>
    </div>
  </header>

  <!-- ===== Focus Mode Exit ===== -->
  <button id="focus-exit" class="fixed bottom-6 right-6 z-[60] items-center gap-2 px-4 py-2 rounded-full text-sm font-semibold shadow-lg"
          style="background: var(--color-accent); color: #fff;">
    <svg class="w-4 h-4 inline" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path d="M8 3v3a2 2 0 01-2 2H3m18 0h-3a2 2 0 01-2-2V3m0 18v-3a2 2 0 012-2h3M3 16h3a2 2 0 012 2v3"/>
    </svg>
    Exit Focus
  </button>

  <!-- ===== Tool Sidebar ===== -->
  <aside id="tool-sidebar" class="fixed top-0 right-0 h-full z-[55] flex" style="background: var(--color-surface); border-left: 1px solid var(--color-border);">
    <div class="sidebar-handle flex-shrink-0"></div>
    <div class="flex-1 flex flex-col overflow-hidden">
      <div class="flex items-center justify-between px-4 py-3" style="border-bottom: 1px solid var(--color-border);">
        <h3 id="sidebar-title" class="font-display font-bold text-sm truncate">Tool Output</h3>
        <button id="sidebar-close" class="p-1 rounded" style="color: var(--color-muted);">
          <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path d="M6 18L18 6M6 6l12 12"/></svg>
        </button>
      </div>
      <div id="sidebar-content" class="flex-1 overflow-y-auto p-4 md-rendered text-sm feed-area"></div>
    </div>
  </aside>

  <!-- ===== Main Content ===== -->
  <main id="main-content" class="flex-1" style="height: calc(100vh - 3.5rem);">

    <!-- ── Connect Tab ── -->
    <section class="tab-panel active flex-col items-center justify-center p-6 sm:p-10 h-full overflow-y-auto"
             data-tabs-target="panel" data-tab="connect">
      <div class="w-full max-w-md mx-auto">
        <div class="backdrop-blur-lg rounded-3xl shadow-lifted p-8" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <div class="mb-8">
            <h2 class="font-display font-bold text-2xl mb-2">Connect to Gateway</h2>
            <p class="text-sm leading-relaxed" style="color: var(--color-muted);">Authenticate with a token or password to open a WebSocket control channel.</p>
          </div>
          <div class="space-y-5">
            <label class="block">
              <span class="text-sm font-medium mb-1.5 block" style="color: var(--color-muted);">Auth Token</span>
              <input id="auth-token" type="password" placeholder="Paste gateway token"
                     class="w-full rounded-xl px-4 py-3 text-sm font-body" style="background: var(--color-surface); border: 1px solid var(--color-border-strong); color: var(--color-ink);">
            </label>
            <label class="block">
              <span class="text-sm font-medium mb-1.5 block" style="color: var(--color-muted);">Password</span>
              <input id="auth-password" type="password" placeholder="Or use password auth"
                     class="w-full rounded-xl px-4 py-3 text-sm font-body" style="background: var(--color-surface); border: 1px solid var(--color-border-strong); color: var(--color-ink);">
            </label>
            <button id="connect-btn"
                    class="w-full font-bold text-sm py-3.5 rounded-xl transition-all duration-150"
                    style="background: linear-gradient(135deg, var(--color-accent-dark), var(--color-accent)); color: #fff;">
              Connect
            </button>
          </div>
          <p class="mt-5 text-xs leading-relaxed" style="color: var(--color-muted);">
            For loopback with no auth configured, leave both fields empty and click Connect.
          </p>
        </div>
      </div>
    </section>

    <!-- ── Chat Tab ── -->
    <section class="tab-panel flex-col h-full" data-tabs-target="panel" data-tab="chat">

      <!-- Config bar -->
      <div class="shrink-0 px-4 sm:px-6 py-2.5 flex items-center gap-3 flex-wrap" style="border-bottom: 1px solid var(--color-border); background: var(--color-panel);">
        <input id="chat-agent" placeholder="Agent (default)"
               class="rounded-lg px-3 py-1.5 text-sm w-32 font-body" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
        <input id="chat-session" placeholder="Session key"
               class="rounded-lg px-3 py-1.5 text-sm w-48 font-mono text-xs" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
        <button id="reset-session-btn"
                class="text-xs font-semibold px-3 py-1.5 rounded-lg transition-colors"
                style="color: var(--color-muted); border: 1px solid var(--color-border); background: var(--color-surface);">
          Reset Session
        </button>
        <button id="refresh-btn"
                class="text-xs font-semibold px-3 py-1.5 rounded-lg transition-colors ml-auto"
                style="color: var(--color-muted); border: 1px solid var(--color-border); background: var(--color-surface);">
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
      <div id="chat-uploads" class="shrink-0 hidden px-4 sm:px-6 py-2" style="border-top: 1px solid var(--color-border); background: var(--color-accent-soft);">
      </div>

      <!-- Input bar -->
      <div class="shrink-0 px-4 sm:px-6 py-3" style="border-top: 1px solid var(--color-border-strong); background: var(--color-panel);">
        <div class="max-w-3xl mx-auto flex gap-3 items-end">
          <div class="flex-1 min-w-0">
            <textarea id="chat-message" rows="1" placeholder="Send a message... (Enter to send, Shift+Enter for newline)"
                      class="w-full rounded-2xl px-4 py-3 text-sm resize-none font-body leading-relaxed max-h-40 overflow-y-auto"
                      style="border: 1px solid var(--color-border-strong); background: var(--color-surface); color: var(--color-ink);"></textarea>
            <div class="mt-1.5 flex items-center gap-3">
              <label class="inline-flex items-center gap-1.5 text-xs font-medium cursor-pointer transition-colors" style="color: var(--color-muted);">
                <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M18.375 12.739l-7.693 7.693a4.5 4.5 0 01-6.364-6.364l10.94-10.94A3 3 0 1119.5 7.372L8.552 18.32m.009-.01l-.01.01m5.699-9.941l-7.81 7.81a1.5 1.5 0 002.112 2.13" />
                </svg>
                Attach
                <input id="chat-attachments" type="file" multiple class="hidden">
              </label>
            </div>
          </div>
          <button id="send-btn"
                  class="shrink-0 font-bold text-sm w-12 h-12 rounded-2xl flex items-center justify-center transition-all duration-150"
                  style="background: linear-gradient(135deg, var(--color-accent-dark), var(--color-accent)); color: #fff;">
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
        <div class="backdrop-blur-lg rounded-2xl shadow-glass p-6 space-y-4" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <h2 class="font-display font-bold text-lg">Canvas Editor</h2>
          <div class="grid grid-cols-2 gap-3">
            <label class="block">
              <span class="text-xs font-medium mb-1 block" style="color: var(--color-muted);">Title</span>
              <input id="canvas-title" placeholder="Untitled"
                     class="w-full rounded-lg px-3 py-2 text-sm font-body" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
            </label>
            <label class="block">
              <span class="text-xs font-medium mb-1 block" style="color: var(--color-muted);">Canvas ID</span>
              <input id="canvas-id" placeholder="main"
                     class="w-full rounded-lg px-3 py-2 text-sm font-mono" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
            </label>
          </div>
          <label class="block">
            <span class="text-xs font-medium mb-1 block" style="color: var(--color-muted);">Session Key</span>
            <input id="canvas-session" placeholder="Link to session"
                   class="w-full rounded-lg px-3 py-2 text-sm font-mono" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
          </label>
          <label class="block">
            <span class="text-xs font-medium mb-1 block" style="color: var(--color-muted);">Body</span>
            <textarea id="canvas-body-input" rows="5" placeholder="Canvas body content"
                      class="w-full rounded-lg px-3 py-2 text-sm font-body resize-vertical" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);"></textarea>
          </label>
          <div class="flex gap-2">
            <button id="canvas-push-btn"
                    class="flex-1 font-semibold text-sm py-2.5 rounded-xl transition-all duration-150"
                    style="background: linear-gradient(135deg, var(--color-accent-dark), var(--color-accent)); color: #fff;">
              Push Canvas
            </button>
            <button id="canvas-append-btn"
                    class="flex-1 font-semibold text-sm py-2.5 rounded-xl transition-colors"
                    style="background: var(--color-surface); border: 1px solid var(--color-border-strong); color: var(--color-ink);">
              Append Block
            </button>
          </div>
          <div class="grid grid-cols-2 gap-3">
            <label class="block">
              <span class="text-xs font-medium mb-1 block" style="color: var(--color-muted);">Block Kind</span>
              <select id="canvas-block-kind"
                      class="w-full rounded-lg px-3 py-2 text-sm font-body" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
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
              <span class="text-xs font-medium mb-1 block" style="color: var(--color-muted);">Block Text</span>
              <input id="canvas-block-text" placeholder="Block content"
                     class="w-full rounded-lg px-3 py-2 text-sm font-body" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
            </label>
          </div>
          <div class="flex gap-2">
            <button id="canvas-export-md-btn"
                    class="flex-1 font-medium text-xs py-2 rounded-lg transition-colors"
                    style="background: var(--color-surface); border: 1px solid var(--color-border-strong); color: var(--color-ink);">
              Export .md
            </button>
            <button id="canvas-export-json-btn"
                    class="flex-1 font-medium text-xs py-2 rounded-lg transition-colors"
                    style="background: var(--color-surface); border: 1px solid var(--color-border-strong); color: var(--color-ink);">
              Export .json
            </button>
            <button id="canvas-clear-btn"
                    class="flex-1 font-medium text-xs py-2 rounded-lg transition-colors"
                    style="color: var(--color-warn); border: 1px solid var(--color-warn); background: var(--color-surface);">
              Clear
            </button>
          </div>
        </div>

        <!-- Preview -->
        <div class="space-y-4">
          <h2 class="font-display font-bold text-lg">Preview</h2>
          <div id="canvas-stage"
               class="min-h-[300px] rounded-2xl p-6 space-y-3" style="background: var(--color-accent-soft); border: 1px solid var(--color-border);">
            <p class="text-sm" style="color: var(--color-muted);">No canvas content yet.</p>
          </div>
        </div>
      </div>
    </section>

    <!-- ── System Tab ── -->
    <section class="tab-panel flex-col h-full overflow-y-auto p-6" data-tabs-target="panel" data-tab="system">
      <div class="max-w-5xl mx-auto w-full grid md:grid-cols-2 gap-6">

        <div class="backdrop-blur-lg rounded-2xl shadow-glass p-6" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <h2 class="font-display font-bold text-lg mb-4">Sessions</h2>
          <div id="sessions-list" class="space-y-2 max-h-72 overflow-y-auto feed-area">
            <p class="text-sm" style="color: var(--color-muted);">No sessions yet.</p>
          </div>
        </div>

        <div class="backdrop-blur-lg rounded-2xl shadow-glass p-6" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <h2 class="font-display font-bold text-lg mb-4">Pairings</h2>
          <div id="pairings-list" class="space-y-2 max-h-72 overflow-y-auto feed-area">
            <p class="text-sm" style="color: var(--color-muted);">No pending pairings.</p>
          </div>
        </div>

        <div class="backdrop-blur-lg rounded-2xl shadow-glass p-6" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <h2 class="font-display font-bold text-lg mb-4">Models</h2>
          <pre id="models-view"
               class="text-xs font-mono rounded-xl p-4 max-h-72 overflow-auto" style="background: var(--color-cream); border: 1px solid var(--color-border);">[]</pre>
        </div>

        <div class="backdrop-blur-lg rounded-2xl shadow-glass p-6" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <h2 class="font-display font-bold text-lg mb-4">Channels</h2>
          <pre id="channels-view"
               class="text-xs font-mono rounded-xl p-4 max-h-72 overflow-auto" style="background: var(--color-cream); border: 1px solid var(--color-border);">[]</pre>
        </div>
      </div>
    </section>

    <!-- ── Usage Tab ── -->
    <section class="tab-panel flex-col h-full overflow-y-auto p-6" data-tabs-target="panel" data-tab="usage">
      <div class="max-w-5xl mx-auto w-full space-y-6">
        <div class="flex items-center justify-between">
          <h2 class="font-display font-bold text-lg">Usage Analytics</h2>
          <button id="usage-export-csv" class="text-xs font-semibold px-3 py-1.5 rounded-lg transition-colors"
                  style="color: var(--color-muted); border: 1px solid var(--color-border); background: var(--color-surface);">
            Export CSV
          </button>
        </div>
        <!-- Stat cards -->
        <div id="usage-totals" class="grid grid-cols-2 md:grid-cols-4 gap-4">
          <div class="rounded-2xl p-4" style="background: var(--color-surface); border: 1px solid var(--color-border);">
            <div class="text-xs font-medium mb-1" style="color: var(--color-muted);">Input Tokens</div>
            <div class="text-2xl font-bold font-mono" id="usage-total-input">0</div>
          </div>
          <div class="rounded-2xl p-4" style="background: var(--color-surface); border: 1px solid var(--color-border);">
            <div class="text-xs font-medium mb-1" style="color: var(--color-muted);">Output Tokens</div>
            <div class="text-2xl font-bold font-mono" id="usage-total-output">0</div>
          </div>
          <div class="rounded-2xl p-4" style="background: var(--color-surface); border: 1px solid var(--color-border);">
            <div class="text-xs font-medium mb-1" style="color: var(--color-muted);">Total Tokens</div>
            <div class="text-2xl font-bold font-mono" id="usage-total-total">0</div>
          </div>
          <div class="rounded-2xl p-4" style="background: var(--color-surface); border: 1px solid var(--color-border);">
            <div class="text-xs font-medium mb-1" style="color: var(--color-muted);">Est. Cost</div>
            <div class="text-2xl font-bold font-mono" id="usage-total-cost">$0.00</div>
          </div>
        </div>
        <!-- Usage table -->
        <div class="rounded-2xl overflow-hidden" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead>
                <tr style="background: var(--color-cream); border-bottom: 1px solid var(--color-border);">
                  <th class="text-left px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Session</th>
                  <th class="text-left px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Channel</th>
                  <th class="text-left px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Model</th>
                  <th class="text-right px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Input</th>
                  <th class="text-right px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Output</th>
                  <th class="text-right px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Turns</th>
                  <th class="text-right px-4 py-2 font-semibold text-xs" style="color: var(--color-muted);">Cost</th>
                </tr>
              </thead>
              <tbody id="usage-table-body">
                <tr><td colspan="7" class="px-4 py-8 text-center" style="color: var(--color-muted);">Connect to load usage data</td></tr>
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </section>

    <!-- ── Agents Tab ── -->
    <section class="tab-panel flex-col h-full overflow-y-auto p-6" data-tabs-target="panel" data-tab="agents">
      <div class="max-w-5xl mx-auto w-full space-y-6">
        <h2 class="font-display font-bold text-lg">Agents</h2>
        <div id="agents-list" class="grid md:grid-cols-2 gap-4">
          <p class="text-sm" style="color: var(--color-muted);">Connect to load agent configuration.</p>
        </div>
      </div>
    </section>

    <!-- ── Cron Tab ── -->
    <section class="tab-panel flex-col h-full overflow-y-auto p-6" data-tabs-target="panel" data-tab="cron">
      <div class="max-w-5xl mx-auto w-full space-y-6">
        <div class="flex items-center justify-between">
          <h2 class="font-display font-bold text-lg">Cron Jobs</h2>
          <button id="cron-refresh" class="text-xs font-semibold px-3 py-1.5 rounded-lg transition-colors"
                  style="color: var(--color-muted); border: 1px solid var(--color-border); background: var(--color-surface);">
            Refresh
          </button>
        </div>
        <!-- Create form -->
        <div class="rounded-2xl p-6 space-y-4" style="background: var(--color-surface); border: 1px solid var(--color-border);">
          <h3 class="font-semibold text-sm">Add Job</h3>
          <div class="grid md:grid-cols-3 gap-3">
            <input id="cron-schedule" placeholder="*/5 * * * * (cron expr)"
                   class="rounded-lg px-3 py-2 text-sm font-mono" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
            <input id="cron-agent" placeholder="Agent ID"
                   class="rounded-lg px-3 py-2 text-sm" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
            <input id="cron-session" placeholder="Session key"
                   class="rounded-lg px-3 py-2 text-sm font-mono" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
          </div>
          <textarea id="cron-prompt" rows="2" placeholder="Prompt text"
                    class="w-full rounded-lg px-3 py-2 text-sm resize-vertical" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);"></textarea>
          <button id="cron-add-btn" class="font-semibold text-sm py-2 px-6 rounded-xl transition-all"
                  style="background: linear-gradient(135deg, var(--color-accent-dark), var(--color-accent)); color: #fff;">
            Add Job
          </button>
        </div>
        <!-- Job list -->
        <div id="cron-list" class="space-y-3">
          <p class="text-sm" style="color: var(--color-muted);">Connect to load cron jobs.</p>
        </div>
      </div>
    </section>

    <!-- ── Logs Tab ── -->
    <section class="tab-panel flex-col h-full" data-tabs-target="panel" data-tab="logs">
      <!-- Toolbar -->
      <div class="shrink-0 px-4 sm:px-6 py-2.5 flex items-center gap-3 flex-wrap" style="border-bottom: 1px solid var(--color-border); background: var(--color-panel);">
        <input id="logs-filter" placeholder="Filter logs..." class="rounded-lg px-3 py-1.5 text-sm w-48" style="border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-ink);">
        <label class="inline-flex items-center gap-1 text-xs font-medium" style="color: var(--color-muted);">
          <input type="checkbox" id="logs-info" checked class="accent-[var(--color-accent)]"> INFO
        </label>
        <label class="inline-flex items-center gap-1 text-xs font-medium" style="color: var(--color-warn);">
          <input type="checkbox" id="logs-warn" checked> WARN
        </label>
        <label class="inline-flex items-center gap-1 text-xs font-medium" style="color: var(--color-danger);">
          <input type="checkbox" id="logs-error" checked> ERROR
        </label>
        <label class="inline-flex items-center gap-1 text-xs font-medium ml-auto" style="color: var(--color-muted);">
          <input type="checkbox" id="logs-autoscroll" checked> Auto-scroll
        </label>
        <button id="logs-clear" class="text-xs font-semibold px-3 py-1.5 rounded-lg"
                style="color: var(--color-warn); border: 1px solid var(--color-warn); background: var(--color-surface);">
          Clear
        </button>
      </div>
      <!-- Log entries -->
      <div class="flex-1 overflow-y-auto min-h-0">
        <div id="logs-feed" class="feed-area font-mono text-xs p-4 space-y-0.5" style="color: var(--color-ink);">
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
      pendingApprovals: [],
      logBuffer: [],
      logBufferMax: 2000,
      usageLoaded: false,
      agentsLoaded: false,
      cronLoaded: false,
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
    // Theme Manager
    // =========================================================================
    const ThemeManager = {
      _pref: document.documentElement._fcThemePref || 'system',
      _mediaQuery: window.matchMedia('(prefers-color-scheme: dark)'),

      init() {
        this._mediaQuery.addEventListener('change', () => {
          if (this._pref === 'system') this._apply();
        });
        document.getElementById('theme-toggle').addEventListener('click', () => this.cycle());
        this._updateIcon();
      },

      cycle() {
        const order = ['system', 'light', 'dark'];
        const idx = order.indexOf(this._pref);
        this._pref = order[(idx + 1) % 3];
        try { localStorage.setItem('frankclaw-theme', this._pref); } catch(_){}
        this._apply();
      },

      _apply() {
        let resolved = this._pref;
        if (resolved === 'system') {
          resolved = this._mediaQuery.matches ? 'dark' : 'light';
        }
        document.documentElement.setAttribute('data-theme', resolved);
        this._updateIcon();
      },

      _updateIcon() {
        document.getElementById('theme-icon-light').classList.toggle('hidden', this._pref !== 'light');
        document.getElementById('theme-icon-dark').classList.toggle('hidden', this._pref !== 'dark');
        document.getElementById('theme-icon-system').classList.toggle('hidden', this._pref !== 'system');
      }
    };

    // =========================================================================
    // Focus Mode
    // =========================================================================
    const FocusMode = {
      _active: false,

      init() {
        try { this._active = localStorage.getItem('frankclaw-focus') === 'true'; } catch(_){}
        if (this._active) this._enter(false);
        document.getElementById('focus-toggle').addEventListener('click', () => this.toggle());
        document.getElementById('focus-exit').addEventListener('click', () => this.toggle());
      },

      toggle() {
        this._active = !this._active;
        if (this._active) this._enter(true);
        else this._exit();
        try { localStorage.setItem('frankclaw-focus', this._active); } catch(_){}
      },

      _enter(switchTab) {
        document.body.classList.add('focus-mode');
        if (switchTab) {
          const tabs = app.getControllerForElementAndIdentifier(
            document.querySelector('[data-controller="tabs"]'), "tabs"
          );
          if (tabs) tabs.show("chat");
        }
      },

      _exit() {
        document.body.classList.remove('focus-mode');
      }
    };

    // =========================================================================
    // Tool Sidebar
    // =========================================================================
    const ToolSidebar = {
      _el: null, _main: null, _resizing: false, _width: 0,

      init() {
        this._el = document.getElementById('tool-sidebar');
        this._main = document.getElementById('main-content');
        document.getElementById('sidebar-close').addEventListener('click', () => this.close());

        // Resize handle
        const handle = this._el.querySelector('.sidebar-handle');
        handle.addEventListener('mousedown', (e) => {
          e.preventDefault();
          this._resizing = true;
          const onMove = (ev) => {
            if (!this._resizing) return;
            const w = Math.max(200, Math.min(window.innerWidth * 0.7, window.innerWidth - ev.clientX));
            this._el.style.width = w + 'px';
            this._main.style.marginRight = w + 'px';
          };
          const onUp = () => { this._resizing = false; document.removeEventListener('mousemove', onMove); document.removeEventListener('mouseup', onUp); };
          document.addEventListener('mousemove', onMove);
          document.addEventListener('mouseup', onUp);
        });
      },

      open(title, markdownContent) {
        document.getElementById('sidebar-title').textContent = title;
        const content = document.getElementById('sidebar-content');
        if (typeof marked !== 'undefined') {
          content.innerHTML = marked.parse(String(markdownContent || ''));
          content.querySelectorAll('a').forEach(a => { a.target = '_blank'; a.rel = 'noreferrer'; });
          content.querySelectorAll('script,iframe,object,embed').forEach(el => el.remove());
        } else {
          content.textContent = markdownContent;
        }
        this._el.classList.add('open');
        this._main.classList.add('sidebar-open');
      },

      close() {
        this._el.classList.remove('open');
        this._el.style.width = '';
        this._main.classList.remove('sidebar-open');
        this._main.style.marginRight = '';
      }
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
        const tab = e.currentTarget.dataset.tab;
        this.show(tab);
        // Lazy-load tab data
        if (tab === 'usage' && !FC.usageLoaded && FC.connected) loadUsage();
        if (tab === 'agents' && !FC.agentsLoaded && FC.connected) loadAgents();
        if (tab === 'cron' && !FC.cronLoaded && FC.connected) loadCron();
      }

      show(name) {
        this.btnTargets.forEach(btn => {
          const active = btn.dataset.tab === name;
          btn.classList.toggle("active", active);
          if (!active) btn.style.color = 'var(--color-muted)';
          else btn.style.color = '';
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
          dot.className = "w-2 h-2 rounded-full " + (connected ? "status-dot" : "");
          dot.style.background = connected ? 'var(--color-accent)' : 'var(--color-warn)';
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
        if (FC._pingInterval) { clearInterval(FC._pingInterval); FC._pingInterval = null; }

        const socket = new WebSocket(FC.buildWsUrl());
        FC.socket = socket;
        FC._reconnectAttempts = 0;

        socket.addEventListener("open", async () => {
          tabs.setStatus("Connected", true);
          tabs.show("chat");
          FC._reconnectAttempts = 0;
          // Reset lazy-load flags
          FC.usageLoaded = false;
          FC.agentsLoaded = false;
          FC.cronLoaded = false;
          // Keepalive ping every 25s to survive proxy idle timeouts.
          if (FC._pingInterval) clearInterval(FC._pingInterval);
          FC._pingInterval = setInterval(() => {
            if (FC.connected) FC.rpc("ping").catch(() => {});
          }, 25000);
          try { await refreshAll(); } catch (e) { appendSystemBubble("error", e.message); }
        });

        socket.addEventListener("message", handleWsMessage);
        socket.addEventListener("close", () => {
          tabs.setStatus("Disconnected", false);
          if (FC._pingInterval) { clearInterval(FC._pingInterval); FC._pingInterval = null; }
          // Auto-reconnect with backoff (max 5 attempts).
          if (FC._reconnectAttempts < 5) {
            const delay = Math.min(1000 * Math.pow(2, FC._reconnectAttempts), 15000);
            FC._reconnectAttempts++;
            tabs.setStatus("Reconnecting in " + Math.round(delay/1000) + "s\u2026", false);
            setTimeout(() => this.doConnect(), delay);
          }
        });
        socket.addEventListener("error", () => tabs.setStatus("Connection error", false));
      }
    });

    // ── Chat ──
    app.register("chat", class extends Controller {
      connect() {
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

        // Paste images
        msg.addEventListener("paste", async (e) => {
          const items = Array.from(e.clipboardData?.items || []);
          const images = items.filter(i => i.type.startsWith("image/"));
          if (!images.length) return;
          e.preventDefault();
          try {
            const files = images.map(i => i.getAsFile()).filter(Boolean);
            await uploadAttachments(files);
            appendSystemBubble("system", "Pasted " + files.length + " image" + (files.length > 1 ? "s" : ""));
          } catch (err) {
            appendSystemBubble("error", err.message);
          }
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

    function mkBtn(label, color, handler) {
      const btn = document.createElement("button");
      btn.className = "text-xs font-semibold px-3 py-1.5 rounded-lg transition-colors";
      btn.style.cssText = "color:" + color + ";border:1px solid " + color + ";background:var(--color-surface);";
      btn.textContent = label;
      btn.addEventListener("click", handler);
      return btn;
    }

    function appendBubble(role, content, attachments = []) {
      const feed = document.getElementById("chat-feed");
      const wrapper = document.createElement("div");
      wrapper.className = "msg-enter";

      if (role === "user") {
        wrapper.className += " flex justify-end";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl rounded-br-sm px-4 py-3 shadow-sm" style="background:var(--color-accent);color:#fff;">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider opacity-60 mb-1">you</div>' +
            '<div class="bubble-content"></div>' +
          '</div>';
      } else if (role === "assistant") {
        wrapper.className += " flex justify-start";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl rounded-bl-sm px-4 py-3 shadow-sm" style="background:var(--color-surface);border:1px solid var(--color-border);">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider mb-1" style="color:var(--color-accent);">assistant</div>' +
            '<div class="bubble-content"></div>' +
          '</div>';
      } else if (role === "tool") {
        wrapper.className += " flex justify-start";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl px-4 py-3 cursor-pointer" style="background:var(--color-cream);border:1px solid var(--color-border);">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider mb-1" style="color:var(--color-muted);">tool result</div>' +
            '<div class="bubble-content"></div>' +
          '</div>';
      } else if (role === "error") {
        wrapper.className += " flex justify-start";
        wrapper.innerHTML =
          '<div class="max-w-[75%] rounded-2xl px-4 py-3" style="background:rgba(220,38,38,0.08);border:1px solid rgba(220,38,38,0.2);">' +
            '<div class="text-[10px] font-semibold uppercase tracking-wider mb-1" style="color:var(--color-danger);">error</div>' +
            '<div class="bubble-content" style="color:var(--color-danger);"></div>' +
          '</div>';
      } else if (role === "approval") {
        wrapper.className += " flex justify-start";
        wrapper.innerHTML =
          '<div class="max-w-[85%] rounded-2xl px-4 py-3" style="background:var(--color-accent-soft);border:1px solid var(--color-accent);">' +
            '<div class="bubble-content"></div>' +
          '</div>';
      } else {
        // system
        wrapper.className += " flex justify-center";
        wrapper.innerHTML =
          '<div class="text-xs px-4 py-1.5 rounded-full" style="color:var(--color-muted);background:var(--color-code-bg);">' +
            '<span class="bubble-content"></span>' +
          '</div>';
      }

      const contentEl = wrapper.querySelector(".bubble-content");
      const useMarkdown = (role === "assistant" || role === "tool");
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
      card.className = "rounded-lg p-3 space-y-2";
      card.style.cssText = "border:1px solid var(--color-border);background:var(--color-cream);";
      const mime = String(att?.mime_type || "application/octet-stream");
      const name = String(att?.filename || att?.media_id || "attachment");
      const url = att?.url || null;

      const label = document.createElement(url ? "a" : "div");
      label.className = "text-sm font-semibold";
      if (url) { label.href = url; label.target = "_blank"; label.rel = "noreferrer"; label.style.color = 'var(--color-accent)'; }
      label.textContent = name;
      card.appendChild(label);

      const meta = document.createElement("div");
      meta.className = "text-[11px] font-mono";
      meta.style.color = 'var(--color-muted)';
      meta.textContent = mime;
      card.appendChild(meta);

      if (url && mime.startsWith("image/")) {
        const img = document.createElement("img");
        img.src = url; img.alt = name; img.loading = "lazy";
        img.className = "max-w-full rounded-lg";
        img.style.border = '1px solid var(--color-border)';
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
          _previewUrl: file.type?.startsWith("image/") ? URL.createObjectURL(file) : null,
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
      el.innerHTML = "";
      for (const a of FC.pendingAttachments) {
        const tag = document.createElement("span");
        tag.className = "inline-flex items-center gap-1.5 text-xs font-medium rounded-full px-3 py-1 mr-2 mb-1";
        tag.style.cssText = "color:var(--color-accent);background:var(--color-accent-soft);";

        if (a._previewUrl) {
          const img = document.createElement("img");
          img.src = a._previewUrl;
          img.className = "w-6 h-6 rounded object-cover";
          tag.appendChild(img);
        } else {
          tag.innerHTML = '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M18.375 12.739l-7.693 7.693a4.5 4.5 0 01-6.364-6.364l10.94-10.94A3 3 0 1119.5 7.372L8.552 18.32"/></svg>';
        }
        const nameSpan = document.createElement("span");
        nameSpan.textContent = a.filename || a.media_id;
        tag.appendChild(nameSpan);

        // Remove button
        const removeBtn = document.createElement("button");
        removeBtn.className = "ml-1 opacity-60 hover:opacity-100";
        removeBtn.innerHTML = '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path d="M6 18L18 6M6 6l12 12"/></svg>';
        removeBtn.addEventListener("click", () => {
          const idx = FC.pendingAttachments.indexOf(a);
          if (idx >= 0) {
            if (a._previewUrl) URL.revokeObjectURL(a._previewUrl);
            FC.pendingAttachments.splice(idx, 1);
            renderPendingAttachments();
          }
        });
        tag.appendChild(removeBtn);
        el.appendChild(tag);
      }
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
        stage.innerHTML = '<p class="text-sm" style="color:var(--color-muted);">No canvas content yet.</p>';
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
      meta.className = "text-[11px] font-mono";
      meta.style.color = 'var(--color-muted)';
      meta.textContent = [canvas.id || "main", canvas.session_key || "no session", "rev " + (canvas.revision || 0), canvas.updated_at || "pending"].join(" \u00b7 ");
      stage.appendChild(meta);

      if (canvas.body) {
        const body = document.createElement("div");
        body.className = "text-sm whitespace-pre-wrap leading-relaxed mt-2";
        const trimmed = canvas.body.trim();
        if (trimmed.startsWith("<svg") || trimmed.startsWith("<?xml")) {
          // Render SVG safely via DOMParser (no script execution).
          try {
            const parser = new DOMParser();
            const doc = parser.parseFromString(trimmed, "image/svg+xml");
            const svg = doc.querySelector("svg");
            if (svg && !doc.querySelector("parsererror")) {
              svg.removeAttribute("width");
              svg.removeAttribute("height");
              svg.style.maxWidth = "100%";
              svg.style.height = "auto";
              body.appendChild(svg);
            } else {
              body.textContent = canvas.body;
            }
          } catch (_) {
            body.textContent = canvas.body;
          }
        } else if (/<[a-z][\s\S]*>/i.test(trimmed)) {
          // Render HTML content in a sandboxed iframe to prevent XSS.
          const iframe = document.createElement("iframe");
          iframe.sandbox = "allow-same-origin";
          iframe.style.cssText = "width:100%;border:none;min-height:200px;background:white;border-radius:8px;";
          iframe.srcdoc = trimmed;
          body.appendChild(iframe);
        } else if (typeof marked !== "undefined") {
          body.innerHTML = marked.parse(canvas.body);
        } else {
          body.textContent = canvas.body;
        }
        stage.appendChild(body);
      }

      for (const block of (canvas.blocks || [])) {
        stage.appendChild(renderCanvasBlock(block));
      }
    }

    function renderCanvasBlock(block) {
      const item = document.createElement("div");
      item.className = "rounded-xl p-3 mt-2";
      item.style.cssText = "border:1px solid var(--color-border);background:var(--color-surface);";
      const kind = block.kind || "block";
      const meta = block.meta || {};

      if (kind === "action") {
        item.innerHTML =
          '<div class="text-[10px] font-semibold uppercase tracking-wider mb-2" style="color:var(--color-muted);">action \u00b7 ' + (meta.action || "noop") + '</div>';
        const btn = document.createElement("button");
        btn.className = "font-semibold text-sm py-2 px-4 rounded-lg transition-colors";
        btn.style.cssText = "background:var(--color-surface);border:1px solid var(--color-border-strong);color:var(--color-ink);";
        btn.textContent = block.text || meta.label || "Run action";
        btn.addEventListener("click", () => runCanvasAction(meta).catch(e => appendSystemBubble("error", e.message)));
        item.appendChild(btn);
        return item;
      }

      const label = kind === "status" ? "status \u00b7 " + (meta.level || "info") : kind === "metric" ? "metric" : kind;
      item.innerHTML = '<div class="text-[10px] font-semibold uppercase tracking-wider mb-1" style="color:var(--color-muted);">' + label + '</div>';

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

    function tokenCount(item) {
      const m = item?.metadata;
      if (!m) return null;
      const input = m.total_input_tokens || m.input_tokens || 0;
      const output = m.total_output_tokens || m.output_tokens || 0;
      return input + output > 0 ? { input, output, total: input + output } : null;
    }

    function renderSessions(items) {
      const el = document.getElementById("sessions-list");
      if (!items.length) {
        el.innerHTML = '<p class="text-sm" style="color:var(--color-muted);">No sessions yet.</p>';
        return;
      }
      el.innerHTML = "";
      for (const item of items) {
        const card = document.createElement("div");
        card.className = "p-3 rounded-xl text-sm";
        card.style.cssText = "border:1px solid var(--color-border);background:var(--color-surface);";

        const header = document.createElement("div");
        header.className = "flex items-center justify-between gap-2";
        header.innerHTML =
          '<div>' +
            '<div class="font-semibold">' + (item.channel || "?") + " / " + (item.account_id || "?") + '</div>' +
            '<div class="font-mono text-[11px] mt-0.5 truncate" style="color:var(--color-muted);">' + item.key + '</div>' +
          '</div>';

        const tc = tokenCount(item);
        if (tc) {
          const badge = document.createElement("span");
          badge.className = "text-[10px] font-mono px-2 py-0.5 rounded-full whitespace-nowrap";
          badge.style.cssText = "background:var(--color-accent-soft);color:var(--color-accent);";
          badge.textContent = tc.total.toLocaleString() + " tok";
          header.appendChild(badge);
        }

        card.appendChild(header);

        const actions = document.createElement("div");
        actions.className = "flex gap-2 mt-2";

        actions.appendChild(mkBtn("Load", "var(--color-accent)", async () => {
          await loadSession(item.key, item.agent_id || null);
          document.querySelector('[data-tab="chat"]').click();
        }));

        actions.appendChild(mkBtn("Compact", "var(--color-muted)", async () => {
          try {
            const r = await FC.rpc("sessions_compact", { session_key: item.key });
            appendSystemBubble("system", "Compacted: pruned " + (r.pruned_count || 0) + " entries");
            await refreshAll();
          } catch(e) { appendSystemBubble("error", e.message); }
        }));

        actions.appendChild(mkBtn("Delete", "var(--color-danger)", async () => {
          if (!confirm("Delete session " + item.key + "?")) return;
          try {
            await FC.rpc("sessions_delete", { session_key: item.key });
            appendSystemBubble("system", "Deleted session " + item.key);
            await refreshAll();
          } catch(e) { appendSystemBubble("error", e.message); }
        }));

        card.appendChild(actions);
        el.appendChild(card);
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
        el.innerHTML = '<p class="text-sm" style="color:var(--color-muted);">No pending pairings.</p>';
        return;
      }
      el.innerHTML = "";
      for (const item of items) {
        const btn = document.createElement("button");
        btn.className = "w-full text-left p-3 rounded-xl transition-all text-sm";
        btn.style.cssText = "border:1px solid var(--color-accent);background:var(--color-accent-soft);";
        btn.innerHTML =
          '<div class="font-semibold" style="color:var(--color-accent);">' + item.channel + " / " + item.account_id + '</div>' +
          '<div class="font-mono text-[11px] mt-0.5" style="color:var(--color-muted);">' + item.sender_id + ' \u00b7 ' + item.code + '</div>' +
          '<div class="text-[11px] font-medium mt-1" style="color:var(--color-accent);">Click to approve</div>';
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
    // Usage Analytics
    // =========================================================================
    async function loadUsage() {
      try {
        const resp = await FC.rpc("usage_get", {});
        FC.usageLoaded = true;
        const totals = resp.totals || {};
        document.getElementById("usage-total-input").textContent = (totals.input_tokens || 0).toLocaleString();
        document.getElementById("usage-total-output").textContent = (totals.output_tokens || 0).toLocaleString();
        document.getElementById("usage-total-total").textContent = ((totals.input_tokens || 0) + (totals.output_tokens || 0)).toLocaleString();
        document.getElementById("usage-total-cost").textContent = "$" + (totals.estimated_cost || 0).toFixed(4);

        const tbody = document.getElementById("usage-table-body");
        const sessions = resp.sessions || [];
        if (!sessions.length) {
          tbody.innerHTML = '<tr><td colspan="7" class="px-4 py-8 text-center" style="color:var(--color-muted);">No usage data</td></tr>';
          return;
        }
        tbody.innerHTML = "";
        for (const s of sessions) {
          const tr = document.createElement("tr");
          tr.style.borderBottom = "1px solid var(--color-border)";
          tr.innerHTML =
            '<td class="px-4 py-2 font-mono text-xs truncate max-w-[200px]">' + (s.session_key || "?") + '</td>' +
            '<td class="px-4 py-2">' + (s.channel || "-") + '</td>' +
            '<td class="px-4 py-2 font-mono text-xs">' + (s.model || "-") + '</td>' +
            '<td class="px-4 py-2 text-right font-mono">' + (s.input_tokens || 0).toLocaleString() + '</td>' +
            '<td class="px-4 py-2 text-right font-mono">' + (s.output_tokens || 0).toLocaleString() + '</td>' +
            '<td class="px-4 py-2 text-right font-mono">' + (s.turns || 0) + '</td>' +
            '<td class="px-4 py-2 text-right font-mono">$' + (s.estimated_cost || 0).toFixed(4) + '</td>';
          tbody.appendChild(tr);
        }
      } catch (e) {
        appendSystemBubble("error", "Usage load failed: " + e.message);
      }
    }

    // =========================================================================
    // Agent Management
    // =========================================================================
    async function loadAgents() {
      try {
        const resp = await FC.rpc("config_get", {});
        FC.agentsLoaded = true;
        const config = resp || {};
        const agents = config.agents?.agents || {};
        const defaultAgent = config.agents?.default_agent || "default";
        const el = document.getElementById("agents-list");

        if (!Object.keys(agents).length) {
          el.innerHTML = '<p class="text-sm" style="color:var(--color-muted);">No agents configured.</p>';
          return;
        }

        el.innerHTML = "";
        for (const [id, agent] of Object.entries(agents)) {
          const card = document.createElement("div");
          card.className = "rounded-2xl p-5 space-y-3";
          card.style.cssText = "background:var(--color-surface);border:1px solid var(--color-border);";

          const header = document.createElement("div");
          header.className = "flex items-center justify-between";
          const nameEl = document.createElement("div");
          nameEl.className = "font-semibold";
          nameEl.textContent = agent.name || id;
          header.appendChild(nameEl);

          if (id === defaultAgent) {
            const badge = document.createElement("span");
            badge.className = "text-[10px] font-semibold px-2 py-0.5 rounded-full";
            badge.style.cssText = "background:var(--color-accent-soft);color:var(--color-accent);";
            badge.textContent = "DEFAULT";
            header.appendChild(badge);
          }
          card.appendChild(header);

          const meta = document.createElement("div");
          meta.className = "text-xs font-mono";
          meta.style.color = "var(--color-muted)";
          meta.textContent = "ID: " + id + (agent.model ? " \u00b7 Model: " + agent.model : "");
          card.appendChild(meta);

          if (agent.tools?.length) {
            const toolsDiv = document.createElement("div");
            toolsDiv.className = "flex flex-wrap gap-1";
            for (const t of agent.tools.slice(0, 10)) {
              const pill = document.createElement("span");
              pill.className = "text-[10px] font-mono px-2 py-0.5 rounded-full";
              pill.style.cssText = "background:var(--color-cream);border:1px solid var(--color-border);";
              pill.textContent = t;
              toolsDiv.appendChild(pill);
            }
            if (agent.tools.length > 10) {
              const more = document.createElement("span");
              more.className = "text-[10px]";
              more.style.color = "var(--color-muted)";
              more.textContent = "+" + (agent.tools.length - 10) + " more";
              toolsDiv.appendChild(more);
            }
            card.appendChild(toolsDiv);
          }

          if (agent.system_prompt) {
            const toggle = document.createElement("details");
            const summary = document.createElement("summary");
            summary.className = "text-xs font-medium cursor-pointer";
            summary.style.color = "var(--color-muted)";
            summary.textContent = "System prompt";
            toggle.appendChild(summary);
            const pre = document.createElement("pre");
            pre.className = "text-xs font-mono mt-2 p-3 rounded-lg overflow-auto max-h-40 whitespace-pre-wrap";
            pre.style.cssText = "background:var(--color-cream);border:1px solid var(--color-border);";
            pre.textContent = String(agent.system_prompt).slice(0, 500) + (agent.system_prompt.length > 500 ? "\n..." : "");
            toggle.appendChild(pre);
            card.appendChild(toggle);
          }

          el.appendChild(card);
        }
      } catch (e) {
        appendSystemBubble("error", "Agents load failed: " + e.message);
      }
    }

    // =========================================================================
    // Cron Management
    // =========================================================================
    async function loadCron() {
      try {
        const resp = await FC.rpc("cron_list", {});
        FC.cronLoaded = true;
        const jobs = resp.jobs || [];
        const el = document.getElementById("cron-list");

        if (!jobs.length) {
          el.innerHTML = '<p class="text-sm" style="color:var(--color-muted);">No cron jobs configured.</p>';
          return;
        }

        el.innerHTML = "";
        for (const job of jobs) {
          const card = document.createElement("div");
          card.className = "rounded-2xl p-4 flex items-start justify-between gap-4";
          card.style.cssText = "background:var(--color-surface);border:1px solid var(--color-border);";

          const info = document.createElement("div");
          info.className = "flex-1 min-w-0";
          info.innerHTML =
            '<div class="font-semibold text-sm">' + (job.label || job.id || "Job") + '</div>' +
            '<div class="font-mono text-xs mt-0.5" style="color:var(--color-muted);">' + (job.schedule || "?") +
              (job.agent_id ? ' \u00b7 ' + job.agent_id : '') + '</div>' +
            '<div class="text-xs mt-1 truncate" style="color:var(--color-muted);">' + (job.prompt || "").slice(0, 100) + '</div>' +
            (job.last_run ? '<div class="text-[10px] mt-1 font-mono" style="color:var(--color-muted);">Last: ' + job.last_run + '</div>' : '');
          card.appendChild(info);

          const actions = document.createElement("div");
          actions.className = "flex gap-2 shrink-0";
          actions.appendChild(mkBtn("Run", "var(--color-accent)", async () => {
            try { await FC.rpc("cron_run", { id: job.id }); appendSystemBubble("system", "Triggered: " + (job.label || job.id)); } catch(e) { appendSystemBubble("error", e.message); }
          }));
          actions.appendChild(mkBtn("Delete", "var(--color-danger)", async () => {
            if (!confirm("Delete job " + (job.label || job.id) + "?")) return;
            try { await FC.rpc("cron_remove", { id: job.id }); await loadCron(); } catch(e) { appendSystemBubble("error", e.message); }
          }));
          card.appendChild(actions);
          el.appendChild(card);
        }
      } catch (e) {
        // Cron RPCs may not be wired yet — show graceful message
        document.getElementById("cron-list").innerHTML = '<p class="text-sm" style="color:var(--color-muted);">Cron service not available.</p>';
      }
    }

    // =========================================================================
    // Logs Viewer
    // =========================================================================
    function appendLogEntry(entry) {
      FC.logBuffer.push(entry);
      if (FC.logBuffer.length > FC.logBufferMax) FC.logBuffer.shift();
      if (shouldShowLog(entry)) renderLogLine(entry);
    }

    function shouldShowLog(entry) {
      const level = String(entry.level || "info").toLowerCase();
      if (level === "info" && !document.getElementById("logs-info").checked) return false;
      if (level === "warn" && !document.getElementById("logs-warn").checked) return false;
      if (level === "error" && !document.getElementById("logs-error").checked) return false;
      const filter = document.getElementById("logs-filter").value.trim().toLowerCase();
      if (filter && !String(entry.message || "").toLowerCase().includes(filter) && !String(entry.target || "").toLowerCase().includes(filter)) return false;
      return true;
    }

    function renderLogLine(entry) {
      const feed = document.getElementById("logs-feed");
      const line = document.createElement("div");
      line.className = "py-0.5 flex gap-2";
      const level = String(entry.level || "info").toLowerCase();
      const color = level === "error" ? "var(--color-danger)" : level === "warn" ? "var(--color-warn)" : "var(--color-muted)";
      const ts = entry.timestamp ? new Date(entry.timestamp).toLocaleTimeString() : "";
      line.innerHTML =
        '<span style="color:var(--color-muted);" class="shrink-0">' + ts + '</span>' +
        '<span style="color:' + color + ';font-weight:600;" class="shrink-0 w-12 text-right">' + level.toUpperCase() + '</span>' +
        '<span class="truncate">' + (entry.target ? '<span style="color:var(--color-accent);">[' + entry.target + ']</span> ' : '') + (entry.message || "") + '</span>';
      feed.appendChild(line);
      if (document.getElementById("logs-autoscroll").checked) {
        feed.scrollTop = feed.scrollHeight;
      }
    }

    function rerenderLogs() {
      const feed = document.getElementById("logs-feed");
      feed.innerHTML = "";
      for (const entry of FC.logBuffer) {
        if (shouldShowLog(entry)) renderLogLine(entry);
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

      if (frame.event === "tool_approval_requested") {
        const p = frame.payload || {};
        const aid = p.approval_id;
        if (!aid) return;
        FC.pendingApprovals.push(aid);
        const bubble = appendBubble("approval", "");
        const root = bubble.querySelector(".bubble-content");
        root.innerHTML = "";

        const header = document.createElement("div");
        header.className = "flex items-center gap-2 mb-2";
        header.innerHTML =
          '<span class="text-[10px] font-semibold uppercase tracking-wider" style="color:var(--color-accent);">Tool Approval</span>' +
          '<span class="text-xs font-mono font-semibold">' + (p.tool_name || "?") + '</span>' +
          (p.risk_level ? '<span class="text-[10px] px-1.5 py-0.5 rounded-full font-semibold" style="background:' +
            (p.risk_level === 'high' ? 'rgba(220,38,38,0.1);color:var(--color-danger)' : 'var(--color-accent-soft);color:var(--color-accent)') +
            ';">' + p.risk_level.toUpperCase() + '</span>' : '');
        root.appendChild(header);

        if (p.arguments) {
          const details = document.createElement("details");
          details.className = "mb-2";
          const summary = document.createElement("summary");
          summary.className = "text-xs cursor-pointer";
          summary.style.color = "var(--color-muted)";
          summary.textContent = "Arguments";
          details.appendChild(summary);
          const pre = document.createElement("pre");
          pre.className = "text-xs font-mono mt-1 p-2 rounded-lg overflow-auto max-h-32";
          pre.style.cssText = "background:var(--color-cream);border:1px solid var(--color-border);";
          pre.textContent = typeof p.arguments === "string" ? p.arguments : JSON.stringify(p.arguments, null, 2);
          details.appendChild(pre);
          root.appendChild(details);
        }

        const btns = document.createElement("div");
        btns.className = "flex gap-2";
        btns.id = "approval-btns-" + aid;

        const resolve = async (decision) => {
          try {
            await FC.rpc("tool_approval_resolve", { approval_id: aid, decision });
            const b = document.getElementById("approval-btns-" + aid);
            if (b) {
              b.innerHTML = '<span class="text-xs font-semibold" style="color:var(--color-muted);">' + decision + '</span>';
            }
            FC.pendingApprovals = FC.pendingApprovals.filter(x => x !== aid);
          } catch(e) { appendSystemBubble("error", e.message); }
        };

        btns.appendChild(mkBtn("Allow Once", "var(--color-accent)", () => resolve("allow_once")));
        btns.appendChild(mkBtn("Allow Always", "var(--color-accent-dark)", () => resolve("allow_always")));
        btns.appendChild(mkBtn("Deny", "var(--color-danger)", () => resolve("deny")));
        root.appendChild(btns);
        return;
      }

      if (frame.event === "tool_approval_resolved") {
        const p = frame.payload || {};
        const btns = document.getElementById("approval-btns-" + p.approval_id);
        if (btns) {
          btns.innerHTML = '<span class="text-xs font-semibold" style="color:var(--color-muted);">' + (p.decision || "resolved") + '</span>';
        }
        return;
      }

      if (frame.event === "canvas_updated") {
        if (frame.payload?.canvas) renderCanvas(frame.payload.canvas);
        else if ((frame.payload?.canvas_id || "main") === (document.getElementById("canvas-id").value.trim() || "main")) renderCanvas(null);
        return;
      }

      if (frame.event === "log_entry") {
        appendLogEntry(frame.payload || {});
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

    // Init subsystems
    ThemeManager.init();
    FocusMode.init();
    ToolSidebar.init();
    renderPendingAttachments();

    // Logs event wiring
    document.getElementById("logs-filter").addEventListener("input", rerenderLogs);
    document.getElementById("logs-info").addEventListener("change", rerenderLogs);
    document.getElementById("logs-warn").addEventListener("change", rerenderLogs);
    document.getElementById("logs-error").addEventListener("change", rerenderLogs);
    document.getElementById("logs-clear").addEventListener("click", () => {
      FC.logBuffer = [];
      document.getElementById("logs-feed").innerHTML = "";
    });

    // Cron wiring
    document.getElementById("cron-refresh").addEventListener("click", () => loadCron());
    document.getElementById("cron-add-btn").addEventListener("click", async () => {
      const schedule = document.getElementById("cron-schedule").value.trim();
      const prompt = document.getElementById("cron-prompt").value.trim();
      if (!schedule || !prompt) return;
      const params = { schedule, prompt };
      const agentId = document.getElementById("cron-agent").value.trim();
      const sessionKey = document.getElementById("cron-session").value.trim();
      if (agentId) params.agent_id = agentId;
      if (sessionKey) params.session_key = sessionKey;
      try {
        await FC.rpc("cron_add", params);
        document.getElementById("cron-schedule").value = "";
        document.getElementById("cron-prompt").value = "";
        document.getElementById("cron-agent").value = "";
        document.getElementById("cron-session").value = "";
        await loadCron();
      } catch(e) { appendSystemBubble("error", e.message); }
    });

    // Usage CSV export
    document.getElementById("usage-export-csv").addEventListener("click", async () => {
      if (!FC.usageLoaded) await loadUsage();
      try {
        const resp = await FC.rpc("usage_get", {});
        const sessions = resp.sessions || [];
        let csv = "session_key,channel,model,input_tokens,output_tokens,turns,estimated_cost\n";
        for (const s of sessions) {
          csv += [s.session_key, s.channel, s.model, s.input_tokens, s.output_tokens, s.turns, s.estimated_cost].join(",") + "\n";
        }
        FC.downloadFile("frankclaw-usage.csv", "text/csv", csv);
      } catch(e) { appendSystemBubble("error", e.message); }
    });
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
