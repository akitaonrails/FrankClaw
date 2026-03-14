#![forbid(unsafe_code)]
#![doc = "Plugin SDK for extending FrankClaw with custom channels, tools, and memory backends."]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

use frankclaw_core::channel::{ChannelPlugin, InboundMessage};
use frankclaw_core::error::{FrankClawError, Result};
use serde::{Deserialize, Serialize};
use frankclaw_core::types::ChannelId;

/// Registry of loaded plugins.
pub struct PluginRegistry {
    channels: Vec<Arc<dyn ChannelPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
        }
    }

    /// Register a channel plugin.
    pub fn register_channel(&mut self, plugin: Arc<dyn ChannelPlugin>) {
        tracing::info!(channel = %plugin.id(), "registered channel plugin");
        self.channels.push(plugin);
    }

    /// Get a channel plugin by ID.
    pub fn get_channel(&self, id: &ChannelId) -> Option<&Arc<dyn ChannelPlugin>> {
        self.channels.iter().find(|p| p.id() == *id)
    }

    /// List all registered channels.
    pub fn list_channels(&self) -> &[Arc<dyn ChannelPlugin>] {
        &self.channels
    }

    /// Start all registered channels, feeding inbound messages to the provided sender.
    #[expect(clippy::unused_async, reason = "async kept for API consistency with channel plugin lifecycle")]
    pub async fn start_all_channels(
        &self,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();

        for plugin in &self.channels {
            let plugin = plugin.clone();
            let tx = inbound_tx.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = plugin.start(tx).await {
                    tracing::error!(channel = %plugin.id(), error = %e, "channel stopped with error");
                }
            });
            handles.push(handle);
        }

        handles
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillManifest {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub prompt: String,
    pub capabilities: Vec<SkillCapability>,
    pub tools: Vec<String>,
}

impl Default for SkillManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: None,
            prompt: String::new(),
            capabilities: vec![SkillCapability::Prompt],
            tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillCapability {
    Prompt,
    ReadSession,
}

pub fn load_workspace_skills(workspace: &Path, names: &[String]) -> Result<Vec<SkillManifest>> {
    names
        .iter()
        .map(|name| load_workspace_skill(workspace, name))
        .collect()
}

pub fn load_workspace_skill(workspace: &Path, name: &str) -> Result<SkillManifest> {
    validate_skill_name(name)?;
    let path = resolve_skill_manifest_path(workspace, name)?;
    let content = std::fs::read_to_string(&path).map_err(|e| FrankClawError::ConfigIo {
        msg: format!("failed to read skill manifest '{}': {e}", path.display()),
    })?;
    let manifest: SkillManifest = serde_json::from_str(&content).map_err(|e| {
        FrankClawError::ConfigValidation {
            msg: format!("invalid skill manifest '{}': {e}", path.display()),
        }
    })?;
    validate_manifest(name, &manifest)?;
    Ok(manifest)
}

fn validate_skill_name(name: &str) -> Result<()> {
    let valid = !name.trim().is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(FrankClawError::ConfigValidation {
            msg: format!("invalid skill name '{name}'"),
        })
    }
}

fn validate_manifest(name: &str, manifest: &SkillManifest) -> Result<()> {
    if manifest.id.trim().is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("skill '{name}' manifest is missing id"),
        });
    }
    if manifest.id != name {
        return Err(FrankClawError::ConfigValidation {
            msg: format!(
                "skill '{}' manifest id '{}' does not match requested skill name",
                name, manifest.id
            ),
        });
    }
    if manifest.name.trim().is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("skill '{name}' manifest is missing name"),
        });
    }
    if manifest.prompt.trim().is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("skill '{name}' manifest is missing prompt"),
        });
    }
    let capabilities: std::collections::HashSet<_> =
        manifest.capabilities.iter().cloned().collect();
    if capabilities.is_empty() {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("skill '{name}' manifest must declare at least one capability"),
        });
    }
    if !capabilities.contains(&SkillCapability::Prompt) {
        return Err(FrankClawError::ConfigValidation {
            msg: format!("skill '{name}' manifest must declare the 'prompt' capability"),
        });
    }
    for tool in &manifest.tools {
        if tool.trim().is_empty() {
            return Err(FrankClawError::ConfigValidation {
                msg: format!("skill '{name}' declares an empty tool name"),
            });
        }
    }
    for required in required_capabilities_for_tools(&manifest.tools) {
        if !capabilities.contains(&required) {
            return Err(FrankClawError::ConfigValidation {
                msg: format!(
                    "skill '{}' is missing required capability '{}'",
                    name,
                    capability_name(&required)
                ),
            });
        }
    }
    Ok(())
}

fn required_capabilities_for_tools(tools: &[String]) -> std::collections::HashSet<SkillCapability> {
    tools
        .iter()
        .filter_map(|tool| match tool.as_str() {
            "session.inspect" => Some(SkillCapability::ReadSession),
            _ => None,
        })
        .collect()
}

fn capability_name(capability: &SkillCapability) -> &'static str {
    match capability {
        SkillCapability::Prompt => "prompt",
        SkillCapability::ReadSession => "read_session",
    }
}

fn resolve_skill_manifest_path(workspace: &Path, name: &str) -> Result<PathBuf> {
    let candidates = [
        workspace.join(".frankclaw/skills").join(name).join("skill.json"),
        workspace.join("skills").join(name).join("skill.json"),
    ];

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| FrankClawError::ConfigIo {
            msg: format!(
                "skill '{}' not found under '{}' or '{}'",
                name,
                workspace.join(".frankclaw/skills").display(),
                workspace.join("skills").display()
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_workspace_skill_rejects_invalid_name() {
        let err = load_workspace_skill(Path::new("."), "../escape").expect_err("skill should fail");
        assert!(err.to_string().contains("invalid skill name"));
    }

    #[test]
    fn load_workspace_skill_reads_hidden_skill_dir() {
        let root = std::env::temp_dir().join(format!(
            "frankclaw-skill-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should work")
                .as_nanos()
        ));
        let skill_dir = root.join(".frankclaw/skills/briefing");
        std::fs::create_dir_all(&skill_dir).expect("skill dir should exist");
        std::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "id": "briefing",
                "name": "Briefing",
                "prompt": "Summarize clearly.",
                "capabilities": ["prompt", "read_session"],
                "tools": ["session.inspect"]
            })
            .to_string(),
        )
        .expect("skill manifest should write");

        let manifest = load_workspace_skill(&root, "briefing").expect("skill should load");
        assert_eq!(manifest.id, "briefing");
        assert_eq!(manifest.tools, vec!["session.inspect"]);
        assert_eq!(
            manifest.capabilities,
            vec![SkillCapability::Prompt, SkillCapability::ReadSession]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_workspace_skill_rejects_missing_required_capability() {
        let root = std::env::temp_dir().join(format!(
            "frankclaw-skill-capability-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should work")
                .as_nanos()
        ));
        let skill_dir = root.join("skills/briefing");
        std::fs::create_dir_all(&skill_dir).expect("skill dir should exist");
        std::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "id": "briefing",
                "name": "Briefing",
                "prompt": "Summarize clearly.",
                "capabilities": ["prompt"],
                "tools": ["session.inspect"]
            })
            .to_string(),
        )
        .expect("skill manifest should write");

        let err = load_workspace_skill(&root, "briefing").expect_err("skill should fail");
        assert!(err.to_string().contains("missing required capability 'read_session'"));

        let _ = std::fs::remove_dir_all(root);
    }
}
