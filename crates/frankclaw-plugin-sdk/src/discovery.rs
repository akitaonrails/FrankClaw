//! Plugin discovery: scan directories for plugin manifests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::manifest::{PluginManifest, load_plugin_manifest};

/// Where a plugin was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginOrigin {
    /// User-level: `~/.local/share/frankclaw/plugins/`
    User,
    /// Workspace-level: `<workspace>/.frankclaw/plugins/`
    Workspace,
}

/// A plugin found on disk with its metadata.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    pub path: PathBuf,
    pub origin: PluginOrigin,
}

/// Scan plugin directories for `plugin.json` manifests.
///
/// Directories are scanned in order; first match for a given plugin id wins.
/// Each entry in `dirs` is a `(path, origin)` pair.
pub fn discover_plugins(dirs: &[(PathBuf, PluginOrigin)]) -> Vec<DiscoveredPlugin> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut results: Vec<DiscoveredPlugin> = Vec::new();

    for (dir, origin) in dirs {
        if !dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                debug!(dir = %dir.display(), error = %e, "failed to read plugin directory");
                continue;
            }
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            let manifest_path = entry_path.join("plugin.json");
            if !manifest_path.is_file() {
                continue;
            }

            // Prevent path traversal: directory name must not contain separators.
            if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                if name.contains('/') || name.contains('\\') || name.starts_with('.') {
                    debug!(name = name, "skipping suspicious plugin directory name");
                    continue;
                }
            }

            match load_plugin_manifest(&manifest_path) {
                Ok(manifest) => {
                    if seen.contains_key(&manifest.id) {
                        debug!(id = %manifest.id, "duplicate plugin id, keeping first");
                        continue;
                    }
                    seen.insert(manifest.id.clone(), results.len());
                    results.push(DiscoveredPlugin {
                        manifest,
                        path: entry_path,
                        origin: *origin,
                    });
                }
                Err(e) => {
                    debug!(
                        path = %manifest_path.display(),
                        error = %e,
                        "skipping invalid plugin manifest"
                    );
                }
            }
        }
    }

    results
}

/// Build the default plugin search directories.
pub fn default_plugin_dirs(workspace: Option<&Path>) -> Vec<(PathBuf, PluginOrigin)> {
    let mut dirs = Vec::new();

    // Workspace plugins (higher priority).
    if let Some(ws) = workspace {
        dirs.push((ws.join(".frankclaw/plugins"), PluginOrigin::Workspace));
    }

    // User-level plugins.
    if let Some(data_dir) = dirs::data_dir() {
        dirs.push((
            data_dir.join("frankclaw/plugins"),
            PluginOrigin::User,
        ));
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_plugin(dir: &Path, id: &str) {
        let plugin_dir = dir.join(id);
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(
            plugin_dir.join("plugin.json"),
            serde_json::json!({
                "id": id,
                "name": format!("Plugin {id}"),
                "version": "1.0.0"
            })
            .to_string(),
        )
        .expect("write manifest");
    }

    #[test]
    fn discover_finds_plugins() {
        let root = std::env::temp_dir().join(format!("fc-discover-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create root");
        create_test_plugin(&root, "alpha");
        create_test_plugin(&root, "beta");

        let plugins = discover_plugins(&[(root.clone(), PluginOrigin::User)]);
        assert_eq!(plugins.len(), 2);

        let ids: Vec<&str> = plugins.iter().map(|p| p.manifest.id.as_str()).collect();
        assert!(ids.contains(&"alpha"));
        assert!(ids.contains(&"beta"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discover_skips_missing_dir() {
        let plugins = discover_plugins(&[(PathBuf::from("/nonexistent/path"), PluginOrigin::User)]);
        assert!(plugins.is_empty());
    }

    #[test]
    fn discover_deduplicates_by_id() {
        let dir1 = std::env::temp_dir().join(format!("fc-dedup1-{}", std::process::id()));
        let dir2 = std::env::temp_dir().join(format!("fc-dedup2-{}", std::process::id()));
        std::fs::create_dir_all(&dir1).expect("create dir1");
        std::fs::create_dir_all(&dir2).expect("create dir2");
        create_test_plugin(&dir1, "same-id");
        create_test_plugin(&dir2, "same-id");

        let plugins = discover_plugins(&[
            (dir1.clone(), PluginOrigin::Workspace),
            (dir2.clone(), PluginOrigin::User),
        ]);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].origin, PluginOrigin::Workspace);

        let _ = std::fs::remove_dir_all(dir1);
        let _ = std::fs::remove_dir_all(dir2);
    }

    #[test]
    fn discover_skips_hidden_dirs() {
        let root = std::env::temp_dir().join(format!("fc-hidden-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create root");

        // Create a hidden directory plugin (should be skipped).
        let hidden = root.join(".hidden-plugin");
        std::fs::create_dir_all(&hidden).expect("create hidden");
        std::fs::write(
            hidden.join("plugin.json"),
            serde_json::json!({
                "id": "hidden",
                "name": "Hidden",
                "version": "1.0.0"
            })
            .to_string(),
        )
        .expect("write");

        // Create a normal plugin.
        create_test_plugin(&root, "visible");

        let plugins = discover_plugins(&[(root.clone(), PluginOrigin::User)]);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].manifest.id, "visible");

        let _ = std::fs::remove_dir_all(root);
    }
}
