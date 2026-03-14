//! Interactive REPL for direct CLI chat.
//!
//! Provides a `frankclaw chat` command that starts an interactive conversation
//! loop directly against the runtime, without requiring the gateway.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

#![forbid(unsafe_code)]

use std::sync::Arc;

use rust_i18n::t;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Editor, Config, EditMode};
use tokio::sync::mpsc;

use frankclaw_core::model::StreamDelta;
use frankclaw_core::types::{AgentId, SessionKey};
use frankclaw_runtime::{ChatRequest, Runtime};

/// Slash commands recognized by the REPL.
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/quit", "Exit the REPL"),
    ("/exit", "Exit the REPL"),
    ("/clear", "Clear the current session"),
    ("/help", "Show available commands"),
    ("/session", "Show current session key"),
    ("/model", "Show or change the model (usage: /model [id])"),
    ("/think", "Set thinking budget (usage: /think [tokens])"),
];

/// Configuration for the REPL session.
pub struct ReplConfig {
    pub agent_id: Option<AgentId>,
    pub session_key: Option<SessionKey>,
    pub model_id: Option<String>,
    pub thinking_budget: Option<u32>,
}

/// Run the interactive REPL loop.
///
/// Reads lines from stdin using rustyline, sends each to the runtime, and
/// streams the response to stdout. Returns when the user types /quit, /exit,
/// Ctrl-D, or Ctrl-C.
#[expect(clippy::too_many_lines, reason = "REPL event loop with input handling and streaming output")]
pub async fn run_repl(runtime: Arc<Runtime>, config: ReplConfig) -> anyhow::Result<()> {
    use std::io::Write;

    let editor_config = Config::builder()
        .edit_mode(EditMode::Emacs)
        .auto_add_history(true)
        .build();

    let mut editor: Editor<ReplHelper, DefaultHistory> = Editor::with_config(editor_config)?;
    editor.set_helper(Some(ReplHelper));

    // Try to load history from state dir.
    let history_path = history_file_path();
    if let Some(ref path) = history_path {
        let _ = editor.load_history(path);
    }

    let mut session_key = config.session_key;
    let mut model_id = config.model_id;
    let mut thinking_budget = config.thinking_budget;
    let agent_id = config.agent_id;

    println!("{}", t!("repl.welcome"));
    println!("{}", t!("repl.help_hint"));
    println!();

    loop {
        let prompt = if session_key.is_some() { "you> " } else { "you (new)> " };

        let line = match editor.readline(prompt) {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: cancel current input, continue.
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D: exit.
                println!("{}", t!("repl.goodbye"));
                break;
            }
            Err(err) => {
                eprintln!("{}: {err}", t!("repl.error_readline"));
                break;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Handle slash commands.
        if let Some(cmd) = trimmed.strip_prefix('/') {
            let (command, args) = match cmd.split_once(char::is_whitespace) {
                Some((c, a)) => (c, a.trim()),
                None => (cmd, ""),
            };

            match command {
                "quit" | "exit" => {
                    println!("{}", t!("repl.goodbye"));
                    break;
                }
                "clear" => {
                    session_key = None;
                    println!("{}", t!("repl.session_cleared"));
                    continue;
                }
                "help" => {
                    println!();
                    for (cmd, desc) in SLASH_COMMANDS {
                        println!("  {cmd:<12} {desc}");
                    }
                    println!();
                    continue;
                }
                "session" => {
                    match &session_key {
                        Some(key) => println!("{}: {key}", t!("repl.current_session")),
                        None => println!("{}", t!("repl.no_session")),
                    }
                    continue;
                }
                "model" => {
                    if args.is_empty() {
                        match &model_id {
                            Some(id) => println!("{}: {id}", t!("repl.current_model")),
                            None => println!("{}", t!("repl.default_model")),
                        }
                    } else {
                        model_id = Some(args.to_string());
                        println!("{}: {args}", t!("repl.model_set"));
                    }
                    continue;
                }
                "think" => {
                    if args.is_empty() {
                        match thinking_budget {
                            Some(budget) => println!("{}: {budget}", t!("repl.thinking_budget")),
                            None => println!("{}", t!("repl.thinking_disabled")),
                        }
                    } else if args == "off" || args == "0" {
                        thinking_budget = None;
                        println!("{}", t!("repl.thinking_disabled"));
                    } else if let Ok(budget) = args.parse::<u32>() {
                        thinking_budget = Some(budget);
                        println!("{}: {budget}", t!("repl.thinking_budget_set"));
                    } else {
                        println!("{}", t!("repl.thinking_usage"));
                    }
                    continue;
                }
                _ => {
                    println!("{}: /{command}", t!("repl.unknown_command"));
                    continue;
                }
            }
        }

        // Send message to runtime with streaming.
        let (stream_tx, mut stream_rx) = mpsc::channel::<StreamDelta>(64);

        let rt = runtime.clone();
        let request = ChatRequest {
            agent_id: agent_id.clone(),
            session_key: session_key.clone(),
            message: trimmed.to_string(),
            attachments: Vec::new(),
            model_id: model_id.clone(),
            max_tokens: None,
            temperature: None,
            stream_tx: Some(stream_tx),
            thinking_budget,
            channel_id: None,
            channel_capabilities: None,
        };

        // Spawn the chat call so we can stream output concurrently.
        let chat_handle = tokio::spawn(async move { rt.chat(request).await });

        // Print streaming output.
        print!("\nassistant> ");
        let _ = std::io::stdout().flush();

        let mut got_text = false;
        while let Some(delta) = stream_rx.recv().await {
            match delta {
                StreamDelta::Text(text) => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                    got_text = true;
                }
                StreamDelta::Error(err) => {
                    eprintln!("\n{}: {err}", t!("repl.error_stream"));
                    break;
                }
                StreamDelta::Done { .. } => break,
                _ => {} // Tool call deltas — ignore in REPL for now.
            }
        }
        if got_text {
            println!();
        }

        // Wait for the full response to get the session key.
        match chat_handle.await {
            Ok(Ok(response)) => {
                // If streaming didn't produce output (provider doesn't support it),
                // print the full response.
                if !got_text {
                    println!("\nassistant> {}", response.content);
                }
                session_key = Some(response.session_key);
                println!(
                    "\n[{}: {} in / {} out]",
                    response.model_id,
                    response.usage.input_tokens,
                    response.usage.output_tokens,
                );
            }
            Ok(Err(err)) => {
                eprintln!("\n{}: {err}", t!("repl.error_chat"));
            }
            Err(err) => {
                eprintln!("\n{}: {err}", t!("repl.error_internal"));
            }
        }

        println!();
    }

    // Save history.
    if let Some(ref path) = history_path {
        let _ = editor.save_history(path);
    }

    Ok(())
}

/// Determine the history file path in the state directory.
fn history_file_path() -> Option<std::path::PathBuf> {
    let state_dir = std::env::var("FRANKCLAW_STATE_DIR")
        .map(std::path::PathBuf::from)
        .ok()
        .or_else(|| {
            dirs::data_local_dir().map(|d| d.join("frankclaw"))
        })?;
    Some(state_dir.join("repl_history.txt"))
}

/// Rustyline helper for slash-command tab completion.
struct ReplHelper;

impl rustyline::completion::Completer for ReplHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        if !line.starts_with('/') {
            return Ok((pos, vec![]));
        }
        let prefix = &line[..pos];
        let completions: Vec<String> = SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(prefix))
            .map(|(cmd, _)| cmd.to_string())
            .collect();
        Ok((0, completions))
    }
}

impl rustyline::hint::Hinter for ReplHelper {
    type Hint = String;
}

impl rustyline::highlight::Highlighter for ReplHelper {}

impl rustyline::validate::Validator for ReplHelper {}

impl rustyline::Helper for ReplHelper {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_commands_are_defined() {
        assert!(SLASH_COMMANDS.len() >= 7);
        // All commands must start with /
        for (cmd, _desc) in SLASH_COMMANDS {
            assert!(cmd.starts_with('/'), "command should start with /: {cmd}");
        }
    }

    #[test]
    fn repl_helper_completes_slash_commands() {
        let helper = ReplHelper;
        use rustyline::completion::Completer;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (start, completions) = helper.complete("/he", 3, &ctx).unwrap();
        assert_eq!(start, 0);
        assert!(completions.contains(&"/help".to_string()));
    }

    #[test]
    fn repl_helper_no_completions_without_slash() {
        let helper = ReplHelper;
        use rustyline::completion::Completer;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, completions) = helper.complete("hello", 5, &ctx).unwrap();
        assert!(completions.is_empty());
    }

    #[test]
    fn repl_helper_completes_multiple() {
        let helper = ReplHelper;
        use rustyline::completion::Completer;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        // "/e" should match /exit
        let (_, completions) = helper.complete("/e", 2, &ctx).unwrap();
        assert!(completions.contains(&"/exit".to_string()));
    }

    #[test]
    fn repl_helper_no_match_for_unknown() {
        let helper = ReplHelper;
        use rustyline::completion::Completer;
        let history = DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, completions) = helper.complete("/zzz", 4, &ctx).unwrap();
        assert!(completions.is_empty());
    }
}
