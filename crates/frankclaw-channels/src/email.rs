use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::{MessageBuilder, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;
use tracing::{info, warn};

use frankclaw_core::channel::{ChannelPlugin, InboundMessage, ChannelCapabilities, HealthStatus, OutboundMessage, SendResult};
use frankclaw_core::error::Result;
use frankclaw_core::types::ChannelId;

/// Default IMAP port (TLS).
const DEFAULT_IMAP_PORT: u16 = 993;

/// Default SMTP port (STARTTLS).
const DEFAULT_SMTP_PORT: u16 = 587;

/// Default poll interval in seconds.
const DEFAULT_POLL_INTERVAL_SECS: u64 = 30;

/// Maximum email body size to process (1 MB).
const MAX_EMAIL_BODY_BYTES: usize = 0x0010_0000;

/// Email channel adapter using IMAP for inbound and SMTP for outbound.
pub struct EmailChannel {
    imap_server: String,
    imap_port: u16,
    imap_user: String,
    imap_password: SecretString,
    smtp_server: String,
    smtp_port: u16,
    smtp_user: String,
    smtp_password: SecretString,
    smtp_from: String,
    poll_interval: Duration,
    allowed_senders: Vec<String>,
    cancel: Mutex<Option<tokio_util::sync::CancellationToken>>,
}

impl EmailChannel {
    #[expect(clippy::too_many_arguments, reason = "email channel configuration requires many distinct parameters")]
    pub fn new(
        imap_server: String,
        imap_port: u16,
        imap_user: String,
        imap_password: SecretString,
        smtp_server: String,
        smtp_port: u16,
        smtp_user: String,
        smtp_password: SecretString,
        smtp_from: String,
        poll_interval_secs: u64,
        allowed_senders: Vec<String>,
    ) -> Self {
        Self {
            imap_server,
            imap_port: if imap_port == 0 { DEFAULT_IMAP_PORT } else { imap_port },
            imap_user,
            imap_password,
            smtp_server,
            smtp_port: if smtp_port == 0 { DEFAULT_SMTP_PORT } else { smtp_port },
            smtp_user,
            smtp_password,
            smtp_from,
            poll_interval: Duration::from_secs(
                if poll_interval_secs == 0 { DEFAULT_POLL_INTERVAL_SECS } else { poll_interval_secs },
            ),
            allowed_senders: allowed_senders.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
            cancel: Mutex::new(None),
        }
    }

    fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.allowed_senders.is_empty() {
            return true;
        }
        self.allowed_senders
            .iter()
            .any(|allowed| allowed == &sender.to_ascii_lowercase())
    }

    async fn connect_imap(
        &self,
    ) -> Result<
        async_imap::Session<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    > {
        let addr = format!("{}:{}", self.imap_server, self.imap_port);
        let tcp_stream = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| self.channel_err(format!("IMAP TCP connect to {addr} failed: {e}")))?;

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));
        let server_name = rustls_pki_types::ServerName::try_from(self.imap_server.clone())
            .map_err(|e| self.channel_err(format!("invalid IMAP server name: {e}")))?;

        let tls_stream = connector
            .connect(server_name, tcp_stream)
            .await
            .map_err(|e| self.channel_err(format!("IMAP TLS handshake failed: {e}")))?;

        let client = async_imap::Client::new(tls_stream);
        let session = client
            .login(&self.imap_user, self.imap_password.expose_secret())
            .await
            .map_err(|(e, _)| self.channel_err(format!("IMAP login failed: {e}")))?;

        Ok(session)
    }

    async fn poll_loop(
        self: Arc<Self>,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        info!(server = %self.imap_server, port = self.imap_port, "email IMAP polling started");
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    info!("email IMAP polling stopped");
                    return;
                }
                () = tokio::time::sleep(self.poll_interval) => {
                    if let Err(e) = self.poll_once(&inbound_tx).await {
                        warn!(error = %e, "email IMAP poll failed");
                    }
                }
            }
        }
    }

    async fn poll_once(
        &self,
        inbound_tx: &tokio::sync::mpsc::Sender<InboundMessage>,
    ) -> Result<()> {
        use futures_util::StreamExt;

        let mut session = self.connect_imap().await?;

        session.select("INBOX").await.map_err(|e| self.channel_err(format!("IMAP select INBOX failed: {e}")))?;

        let unseen = session.search("UNSEEN").await.map_err(|e| self.channel_err(format!("IMAP search UNSEEN failed: {e}")))?;

        if unseen.is_empty() {
            let _ = session.logout().await;
            return Ok(());
        }

        let seq_set = unseen
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");

        let mut fetch_stream = session.fetch(&seq_set, "RFC822").await.map_err(|e| self.channel_err(format!("IMAP fetch failed: {e}")))?;

        let mut processed_seqs = Vec::new();
        while let Some(fetch_result) = fetch_stream.next().await {
            let fetch = match fetch_result {
                Ok(f) => f,
                Err(e) => {
                    warn!(error = %e, "IMAP fetch item error");
                    continue;
                }
            };

            let Some(body) = fetch.body() else { continue };

            if body.len() > MAX_EMAIL_BODY_BYTES {
                warn!(size = body.len(), "email body exceeds size limit, skipping");
                processed_seqs.push(fetch.message);
                continue;
            }

            if let Some(msg) = self.parse_email(body)
                && inbound_tx.send(msg).await.is_err()
            {
                warn!("inbound channel closed, stopping email poll");
                break;
            }
            processed_seqs.push(fetch.message);
        }
        drop(fetch_stream);

        if !processed_seqs.is_empty() {
            let seen_set = processed_seqs
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            if let Err(e) = session.store(&seen_set, "+FLAGS (\\Seen)").await {
                warn!(error = %e, "IMAP store SEEN flag failed");
            }
        }

        let _ = session.logout().await;
        Ok(())
    }

    fn parse_email(&self, raw: &[u8]) -> Option<InboundMessage> {
        let parsed = mail_parser::MessageParser::default().parse(raw)?;

        let from = parsed
            .from()
            .and_then(|addrs| addrs.first())
            .and_then(|addr| addr.address())
            .map(str::to_string)?;

        if !self.is_sender_allowed(&from) {
            warn!(sender = %from, "email from non-allowed sender, skipping");
            return None;
        }

        let sender_name = parsed
            .from()
            .and_then(|addrs| addrs.first())
            .and_then(|addr| addr.name())
            .map(str::to_string);

        let subject = parsed.subject().unwrap_or("").to_string();
        let message_id = parsed.message_id().unwrap_or("").to_string();

        let body_text = extract_body_text(&parsed);
        if body_text.is_empty() {
            return None;
        }

        let text = if subject.is_empty() {
            body_text
        } else {
            format!("Subject: {subject}\n\n{body_text}")
        };

        Some(InboundMessage {
            channel: ChannelId::new("email"),
            account_id: self.smtp_from.clone(),
            sender_id: from,
            sender_name,
            thread_id: Some(message_id),
            is_group: false,
            is_mention: false,
            text: Some(text),
            attachments: Vec::new(),
            platform_message_id: parsed.message_id().map(str::to_string),
            timestamp: parsed
                .date().map_or_else(chrono::Utc::now, |d| {
                    chrono::DateTime::from_timestamp(d.to_timestamp(), 0)
                        .unwrap_or_else(chrono::Utc::now)
                }),
        })
    }
}

fn extract_body_text(parsed: &mail_parser::Message<'_>) -> String {
    if let Some(text) = parsed.body_text(0) {
        return text.to_string();
    }
    if let Some(html) = parsed.body_html(0) {
        return strip_html_tags(&html);
    }
    String::new()
}

fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

fn build_reply_headers(
    builder: MessageBuilder,
    thread_id: Option<&str>,
    subject: &str,
) -> MessageBuilder {
    let builder = builder.subject(if subject.starts_with("Re: ") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    });

    if let Some(msg_id) = thread_id {
        builder
            .in_reply_to(msg_id.to_string())
            .references(msg_id.to_string())
    } else {
        builder
    }
}

#[async_trait]
impl ChannelPlugin for EmailChannel {
    fn id(&self) -> ChannelId {
        ChannelId::new("email")
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            threads: true,
            attachments: false,
            ..Default::default()
        }
    }

    fn label(&self) -> &'static str {
        "Email"
    }

    async fn start(&self, _inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>) -> Result<()> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut guard = self.cancel.lock().await;
        *guard = Some(cancel);
        drop(guard);
        info!(server = %self.imap_server, "email channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut guard = self.cancel.lock().await;
        if let Some(cancel) = guard.take() {
            cancel.cancel();
        }
        info!("email channel stopped");
        Ok(())
    }

    async fn health(&self) -> HealthStatus {
        HealthStatus::Connected
    }

    async fn send(&self, msg: OutboundMessage) -> Result<SendResult> {
        let from: lettre::message::Mailbox = self
            .smtp_from
            .parse()
            .map_err(|e| self.channel_err(format!("invalid smtp_from address: {e}")))?;

        let to: lettre::message::Mailbox = msg
            .to
            .parse()
            .map_err(|e| self.channel_err(format!("invalid recipient address '{}': {e}", msg.to)))?;

        let subject = msg
            .thread_id
            .as_deref()
            .map(|_| "Re: ".to_string())
            .unwrap_or_default();

        let mut builder = lettre::Message::builder().from(from).to(to);

        builder = build_reply_headers(builder, msg.thread_id.as_deref(), &subject);

        let email = builder
            .header(ContentType::TEXT_PLAIN)
            .body(msg.text.clone())
            .map_err(|e| self.channel_err(format!("failed to build email: {e}")))?;

        let creds = Credentials::new(
            self.smtp_user.clone(),
            self.smtp_password.expose_secret().to_string(),
        );

        let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&self.smtp_server)
            .map_err(|e| self.channel_err(format!("SMTP relay setup failed: {e}")))?
            .port(self.smtp_port)
            .credentials(creds)
            .build();

        let response = mailer.send(email).await.map_err(|e| self.channel_err(format!("SMTP send failed: {e}")))?;

        Ok(SendResult::Sent {
            platform_message_id: response.message().collect::<Vec<_>>().join(" "),
        })
    }
}

impl EmailChannel {
    /// Start the IMAP polling loop. Must be called with an Arc<Self>.
    pub fn start_polling(
        self: Arc<Self>,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.poll_loop(inbound_tx, cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_tags_removes_tags() {
        assert_eq!(strip_html_tags("<p>Hello <b>world</b></p>"), "Hello world");
        assert_eq!(strip_html_tags("no tags here"), "no tags here");
        assert_eq!(strip_html_tags("<div><a href=\"x\">link</a></div>"), "link");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn is_sender_allowed_empty_allowlist_accepts_all() {
        let channel = make_test_channel(vec![]);
        assert!(channel.is_sender_allowed("anyone@example.com"));
    }

    #[test]
    fn is_sender_allowed_checks_case_insensitively() {
        let channel = make_test_channel(vec!["Alice@Example.COM".into()]);
        assert!(channel.is_sender_allowed("alice@example.com"));
        assert!(channel.is_sender_allowed("ALICE@EXAMPLE.COM"));
        assert!(!channel.is_sender_allowed("bob@example.com"));
    }

    #[test]
    fn build_reply_headers_adds_re_prefix() {
        let builder = lettre::Message::builder()
            .from("test@example.com".parse().unwrap())
            .to("other@example.com".parse().unwrap());
        let builder =
            build_reply_headers(builder, Some("<msg-123@example.com>"), "Original Subject");
        let email = builder
            .header(ContentType::TEXT_PLAIN)
            .body("test".to_string())
            .expect("email should build");

        let headers = email.headers();
        let subject: &str = headers.get_raw("Subject").expect("subject should exist");
        assert!(subject.contains("Re: "));

        let in_reply_to: &str = headers
            .get_raw("In-Reply-To")
            .expect("in-reply-to should exist");
        assert!(in_reply_to.contains("<msg-123@example.com>"));
    }

    #[test]
    fn build_reply_headers_preserves_existing_re() {
        let builder = lettre::Message::builder()
            .from("test@example.com".parse().unwrap())
            .to("other@example.com".parse().unwrap());
        let builder = build_reply_headers(builder, None, "Re: Already replied");
        let email = builder
            .header(ContentType::TEXT_PLAIN)
            .body("test".to_string())
            .expect("email should build");

        let headers = email.headers();
        let subject: &str = headers.get_raw("Subject").expect("subject should exist");
        assert!(!subject.contains("Re: Re: "));
    }

    #[test]
    fn parse_email_rejects_non_allowed_sender() {
        let channel = make_test_channel(vec!["allowed@example.com".into()]);
        let raw = build_raw_email("denied@example.com", "Test Subject", "Hello world");
        assert!(channel.parse_email(raw.as_bytes()).is_none());
    }

    #[test]
    fn parse_email_extracts_text_body() {
        let channel = make_test_channel(vec![]);
        let raw = build_raw_email("sender@example.com", "Test Subject", "Hello world");
        let msg = channel.parse_email(raw.as_bytes()).expect("should parse");
        assert_eq!(msg.sender_id, "sender@example.com");
        assert!(msg.text.as_ref().unwrap().contains("Hello world"));
        assert!(msg.text.as_ref().unwrap().contains("Test Subject"));
    }

    #[test]
    fn parse_email_returns_none_for_empty_body() {
        let channel = make_test_channel(vec![]);
        let raw = build_raw_email("sender@example.com", "Empty", "");
        assert!(channel.parse_email(raw.as_bytes()).is_none());
    }

    fn make_test_channel(allowed_senders: Vec<String>) -> EmailChannel {
        EmailChannel::new(
            "imap.example.com".into(),
            993,
            "user@example.com".into(),
            SecretString::from("password".to_string()),
            "smtp.example.com".into(),
            587,
            "user@example.com".into(),
            SecretString::from("password".to_string()),
            "user@example.com".into(),
            30,
            allowed_senders,
        )
    }

    fn build_raw_email(from: &str, subject: &str, body: &str) -> String {
        format!(
            "From: {from}\r\n\
             To: user@example.com\r\n\
             Subject: {subject}\r\n\
             Date: Tue, 11 Mar 2026 12:00:00 +0000\r\n\
             Message-ID: <test-123@example.com>\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\
             \r\n\
             {body}"
        )
    }
}
