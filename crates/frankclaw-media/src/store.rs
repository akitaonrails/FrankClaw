use std::path::PathBuf;

use chrono::Utc;
use tracing::debug;

use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::media::MediaFile;
use frankclaw_core::types::MediaId;

/// File-based media store with TTL cleanup.
///
/// Files are stored with owner-only permissions (0o600).
/// Each file gets a UUID to prevent enumeration attacks.
pub struct MediaStore {
    base_dir: PathBuf,
    max_file_size: u64,
    ttl_hours: u64,
}

impl MediaStore {
    pub fn new(base_dir: PathBuf, max_file_size: u64, ttl_hours: u64) -> Result<Self> {
        std::fs::create_dir_all(&base_dir).map_err(|e| FrankClawError::Internal {
            msg: format!("failed to create media directory: {e}"),
        })?;

        // Set directory permissions to owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(&base_dir, perms);
        }

        Ok(Self {
            base_dir,
            max_file_size,
            ttl_hours,
        })
    }

    /// Store bytes as a media file. Returns metadata.
    pub fn store(
        &self,
        original_name: &str,
        mime_type: &str,
        data: &[u8],
    ) -> Result<MediaFile> {
        if data.len() as u64 > self.max_file_size {
            return Err(FrankClawError::MediaTooLarge {
                max_bytes: self.max_file_size,
            });
        }

        let id = MediaId::new();
        let ext = mime_to_safe_extension(mime_type);
        let filename = format!("{id}.{ext}");
        let path = self.base_dir.join(&filename);

        std::fs::write(&path, data).map_err(|e| FrankClawError::Internal {
            msg: format!("failed to write media file: {e}"),
        })?;

        // Set file permissions to owner-only (NOT 0o644 like OpenClaw).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }

        let now = Utc::now();
        Ok(MediaFile {
            id,
            original_name: sanitize_filename(original_name),
            mime_type: mime_type.to_string(),
            size_bytes: data.len() as u64,
            path,
            created_at: now,
            expires_at: now + chrono::Duration::hours(self.ttl_hours as i64),
        })
    }

    /// Delete expired media files.
    pub fn cleanup(&self) -> Result<u64> {
        let mut deleted = 0u64;
        let _now = Utc::now();

        let entries = std::fs::read_dir(&self.base_dir).map_err(|e| FrankClawError::Internal {
            msg: format!("failed to read media directory: {e}"),
        })?;

        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    let age = std::time::SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default();
                    if age > std::time::Duration::from_secs(self.ttl_hours * 3600) {
                        if std::fs::remove_file(entry.path()).is_ok() {
                            deleted += 1;
                        }
                    }
                }
            }
        }

        if deleted > 0 {
            debug!(deleted, "cleaned up expired media files");
        }

        Ok(deleted)
    }
}

/// Map MIME type to a safe file extension.
/// Prevents storing executable extensions that could be accidentally run.
fn mime_to_safe_extension(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "audio/webm" => "weba",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "application/json" => "json",
        _ => "bin", // Safe default — never .exe, .sh, .bat, etc.
    }
}

/// Sanitize filename to prevent path traversal.
/// Strips directory separators, leading dots, and limits length.
fn sanitize_filename(name: &str) -> String {
    // Take only the filename component (strip any directory path).
    let basename = name.rsplit(&['/', '\\']).next().unwrap_or(name);
    // Allow only safe characters.
    let cleaned: String = basename
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .take(255)
        .collect();
    // Strip leading dots to prevent hidden files / traversal.
    cleaned.trim_start_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_traversal() {
        assert_eq!(sanitize_filename("../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("normal-file.txt"), "normal-file.txt");
        assert_eq!(sanitize_filename("file with spaces.png"), "filewithspaces.png");
    }

    #[test]
    fn safe_extensions() {
        assert_eq!(mime_to_safe_extension("application/x-executable"), "bin");
        assert_eq!(mime_to_safe_extension("application/x-sh"), "bin");
        assert_eq!(mime_to_safe_extension("image/png"), "png");
    }
}
