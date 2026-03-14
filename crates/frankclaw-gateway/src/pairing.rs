use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use frankclaw_core::error::{FrankClawError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingPairing {
    pub channel: String,
    pub account_id: String,
    pub sender_id: String,
    pub code: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Default, Serialize, Deserialize)]
struct PairingState {
    approved: HashMap<String, HashSet<String>>,
    pending: Vec<PendingPairing>,
}

pub struct PairingStore {
    path: PathBuf,
    state: Mutex<PairingState>,
}

impl PairingStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| FrankClawError::ConfigIo {
                msg: format!("failed to create pairing directory: {e}"),
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }

        let state = if path.exists() {
            let content = std::fs::read_to_string(path).map_err(|e| FrankClawError::ConfigIo {
                msg: format!("failed to read pairing file: {e}"),
            })?;
            serde_json::from_str(&content).map_err(|e| FrankClawError::ConfigIo {
                msg: format!("failed to parse pairing file: {e}"),
            })?
        } else {
            PairingState::default()
        };

        Ok(Self {
            path: path.to_path_buf(),
            state: Mutex::new(state),
        })
    }

    pub fn is_approved(&self, channel: &str, account_id: &str, sender_id: &str) -> bool {
        let state = self.state.lock().expect("pairing state poisoned");
        state
            .approved
            .get(&approval_key(channel, account_id))
            .is_some_and(|approved| approved.contains(sender_id))
    }

    #[expect(clippy::unwrap_in_result, reason = "mutex poisoning is unrecoverable; propagating the error would not help callers")]
    pub fn ensure_pending(
        &self,
        channel: &str,
        account_id: &str,
        sender_id: &str,
    ) -> Result<PendingPairing> {
        let mut state = self.state.lock().expect("pairing state poisoned");
        if let Some(existing) = state
            .pending
            .iter()
            .find(|pending| {
                pending.channel == channel
                    && pending.account_id == account_id
                    && pending.sender_id == sender_id
            })
            .cloned()
        {
            return Ok(existing);
        }

        let pending = PendingPairing {
            channel: channel.to_string(),
            account_id: account_id.to_string(),
            sender_id: sender_id.to_string(),
            code: generate_code(),
            created_at: chrono::Utc::now(),
        };
        state.pending.push(pending.clone());
        save_state(&self.path, &state)?;
        Ok(pending)
    }

    pub fn list_pending(&self, channel: Option<&str>) -> Vec<PendingPairing> {
        let state = self.state.lock().expect("pairing state poisoned");
        state
            .pending
            .iter()
            .filter(|pending| channel.is_none_or(|value| value == pending.channel))
            .cloned()
            .collect()
    }

    #[expect(clippy::unwrap_in_result, reason = "mutex poisoning is unrecoverable; propagating the error would not help callers")]
    pub fn approve(
        &self,
        channel: Option<&str>,
        account_id: Option<&str>,
        code: &str,
    ) -> Result<PendingPairing> {
        let mut state = self.state.lock().expect("pairing state poisoned");
        let index = state
            .pending
            .iter()
            .position(|pending| {
                pending.code == code
                    && channel.is_none_or(|value| value == pending.channel)
                    && account_id
                        .is_none_or(|value| value == pending.account_id)
            })
            .ok_or_else(|| FrankClawError::ConfigValidation {
                msg: format!("no pending pairing found for code '{code}'"),
            })?;

        let pending = state.pending.remove(index);
        state
            .approved
            .entry(approval_key(&pending.channel, &pending.account_id))
            .or_default()
            .insert(pending.sender_id.clone());
        save_state(&self.path, &state)?;
        Ok(pending)
    }
}

fn approval_key(channel: &str, account_id: &str) -> String {
    format!("{channel}:{account_id}")
}

fn generate_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let bytes = rand::random::<[u8; 6]>();
    bytes
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect()
}

fn save_state(path: &Path, state: &PairingState) -> Result<()> {
    let content = serde_json::to_string_pretty(state).map_err(|e| FrankClawError::ConfigIo {
        msg: format!("failed to serialize pairing state: {e}"),
    })?;
    std::fs::write(path, content).map_err(|e| FrankClawError::ConfigIo {
        msg: format!("failed to write pairing file: {e}"),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_pending_and_approve_roundtrip() {
        let temp = std::env::temp_dir().join(format!(
            "frankclaw-pairing-{}.json",
            uuid::Uuid::new_v4()
        ));
        let store = PairingStore::open(&temp).expect("store should open");

        let pending = store
            .ensure_pending("telegram", "default", "user-1")
            .expect("pending should be created");
        assert_eq!(store.list_pending(Some("telegram")).len(), 1);

        let approved = store
            .approve(Some("telegram"), Some("default"), &pending.code)
            .expect("approval should succeed");
        assert_eq!(approved.sender_id, "user-1");
        assert!(store.is_approved("telegram", "default", "user-1"));

        let _ = std::fs::remove_file(temp);
    }
}
