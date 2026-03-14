use frankclaw_core::channel::OutboundAttachment;
use frankclaw_core::error::{FrankClawError, Result};
use frankclaw_core::media::safe_extension_for_mime;
use frankclaw_core::types::ChannelId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Audio,
    Video,
    Document,
}

pub fn attachment_kind(mime_type: &str) -> AttachmentKind {
    let normalized = mime_type.trim().to_ascii_lowercase();
    if normalized.starts_with("image/") {
        AttachmentKind::Image
    } else if normalized.starts_with("audio/") {
        AttachmentKind::Audio
    } else if normalized.starts_with("video/") {
        AttachmentKind::Video
    } else {
        AttachmentKind::Document
    }
}

pub fn require_single_attachment<'att>(
    channel: &ChannelId,
    attachments: &'att [OutboundAttachment],
) -> Result<&'att OutboundAttachment> {
    match attachments {
        [attachment] => Ok(attachment),
        [] => Err(FrankClawError::Channel {
            channel: channel.clone(),
            msg: "attachment send requested without any attachments".into(),
        }),
        _ => Err(FrankClawError::Channel {
            channel: channel.clone(),
            msg: "multiple attachments are not supported yet on this channel".into(),
        }),
    }
}

pub fn attachment_filename(attachment: &OutboundAttachment) -> String {
    attachment
        .filename
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_filename(&attachment.mime_type))
}

pub fn attachment_bytes(
    channel: &ChannelId,
    attachment: &OutboundAttachment,
) -> Result<Vec<u8>> {
    if attachment.bytes.is_empty() {
        return Err(FrankClawError::Channel {
            channel: channel.clone(),
            msg: format!(
                "attachment {} is missing inline bytes",
                attachment.media_id
            ),
        });
    }

    Ok(attachment.bytes.clone())
}

fn default_filename(mime_type: &str) -> String {
    let ext = safe_extension_for_mime(mime_type);
    format!("attachment.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use frankclaw_core::types::MediaId;

    #[test]
    fn classifies_common_media_types() {
        assert_eq!(attachment_kind("image/png"), AttachmentKind::Image);
        assert_eq!(attachment_kind("audio/ogg"), AttachmentKind::Audio);
        assert_eq!(attachment_kind("video/mp4"), AttachmentKind::Video);
        assert_eq!(
            attachment_kind("application/pdf"),
            AttachmentKind::Document
        );
    }

    #[test]
    fn infers_default_filename_from_media_kind() {
        let attachment = OutboundAttachment {
            media_id: MediaId::new(),
            mime_type: "audio/mp4".into(),
            filename: None,
            url: None,
            bytes: b"voice".to_vec(),
        };

        assert_eq!(attachment_filename(&attachment), "attachment.m4a");
    }
}
