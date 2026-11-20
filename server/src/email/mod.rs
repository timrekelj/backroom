//! Outgoing email: Resend's HTTP API in production, SMTP (Mailpit) in
//! development, and an in-memory outbox for tests.
//!
//! Mail is sent from a spawned task ([`Mailer::send`]) so request latency
//! does not reveal whether a message was sent (enumeration safety) and a slow
//! provider never stalls an RPC.

pub mod templates;

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;

/// A rendered message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
    /// Stable key so a retried send is delivered once (Resend `Idempotency-Key`).
    pub idempotency_key: String,
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    async fn send(&self, from: &str, email: &Email) -> Result<()>;
}

/// Resend HTTP API (`THEHIDEOUT_RESEND_API_KEY`).
pub struct ResendSender {
    api_key: String,
    http: reqwest::Client,
    endpoint: String,
}

impl ResendSender {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("build reqwest client"),
            endpoint: "https://api.resend.com/emails".to_owned(),
        }
    }
}

#[async_trait]
impl EmailSender for ResendSender {
    async fn send(&self, from: &str, email: &Email) -> Result<()> {
        let body = serde_json::json!({
            "from": from,
            "to": [email.to],
            "subject": email.subject,
            "text": email.text,
            "html": email.html,
        });
        let resp = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .header("idempotency-key", &email.idempotency_key)
            .body(serde_json::to_vec(&body)?)
            .send()
            .await
            .context("sending to Resend")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("Resend answered {status}: {}", text.chars().take(300).collect::<String>());
        }
        Ok(())
    }
}

/// Plain SMTP without auth or TLS, for the local Mailpit sink (`THEHIDEOUT_SMTP_URL`).
pub struct SmtpSender {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
}

impl SmtpSender {
    /// `smtp://host:port`.
    pub fn new(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url).with_context(|| format!("invalid SMTP URL {url:?}"))?;
        if parsed.scheme() != "smtp" {
            bail!("SMTP URL must start with smtp:// (got {url:?})");
        }
        let host = parsed.host_str().context("SMTP URL has no host")?.to_owned();
        let port = parsed.port().unwrap_or(25);
        let transport = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(host)
            .port(port)
            .timeout(Some(std::time::Duration::from_secs(10)))
            .build();
        Ok(Self { transport })
    }
}

#[async_trait]
impl EmailSender for SmtpSender {
    async fn send(&self, from: &str, email: &Email) -> Result<()> {
        use lettre::AsyncTransport;
        use lettre::message::{Message, MultiPart};
        let message = Message::builder()
            .from(from.parse().context("invalid THEHIDEOUT_EMAIL_FROM")?)
            .to(email.to.parse().context("invalid recipient")?)
            .subject(&email.subject)
            .message_id(Some(format!("<{}@thehideout>", email.idempotency_key)))
            .multipart(MultiPart::alternative_plain_html(email.text.clone(), email.html.clone()))?;
        self.transport.send(message).await.context("sending over SMTP")?;
        Ok(())
    }
}

/// Keeps every message in memory; the default (tests read [`MemorySender::messages`]).
#[derive(Clone, Default)]
pub struct MemorySender {
    messages: Arc<Mutex<Vec<Email>>>,
}

impl MemorySender {
    pub fn messages(&self) -> Vec<Email> {
        self.messages.lock().unwrap().clone()
    }

    /// Messages sent to `to` (case-insensitive), oldest first.
    pub fn to(&self, to: &str) -> Vec<Email> {
        let to = to.to_lowercase();
        self.messages().into_iter().filter(|m| m.to.to_lowercase() == to).collect()
    }
}

#[async_trait]
impl EmailSender for MemorySender {
    async fn send(&self, _from: &str, email: &Email) -> Result<()> {
        self.messages.lock().unwrap().push(email.clone());
        Ok(())
    }
}

/// The configured sender plus the sender address and the public base URL
/// used in links.
#[derive(Clone)]
pub struct Mailer {
    sender: Arc<dyn EmailSender>,
    pub from: String,
    /// Set when `sender` is the in-memory one, so tests can read it.
    pub outbox: Option<MemorySender>,
    /// Sends still running (see [`Mailer::flush`]).
    pending: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Mailer {
    pub fn memory() -> Self {
        let outbox = MemorySender::default();
        Self {
            sender: Arc::new(outbox.clone()),
            from: DEFAULT_FROM.to_owned(),
            outbox: Some(outbox),
            pending: Arc::default(),
        }
    }

    pub fn new(sender: Arc<dyn EmailSender>, from: String) -> Self {
        Self { sender, from, outbox: None, pending: Arc::default() }
    }

    /// Sends in the background; failures are logged.
    pub fn send(&self, email: Email) {
        let mailer = self.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = mailer.send_now(&email).await {
                tracing::warn!(error = %format!("{e:#}"), subject = %email.subject, "sending email failed");
            }
        });
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|h| !h.is_finished());
        pending.push(handle);
    }

    /// Waits for every background send (the CLI, before exiting).
    pub async fn flush(&self) {
        let handles: Vec<_> = std::mem::take(&mut *self.pending.lock().unwrap());
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Sends and waits (CLI and bootstrap).
    pub async fn send_now(&self, email: &Email) -> Result<()> {
        self.sender.send(&self.from, email).await?;
        tracing::info!(subject = %email.subject, "sent email");
        Ok(())
    }
}

pub const DEFAULT_FROM: &str = "thehideout <noreply@thehideout.zabit.si>";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_sender_records_messages() {
        let mailer = Mailer::memory();
        let email = Email {
            to: "A@x.org".into(),
            subject: "s".into(),
            text: "t".into(),
            html: "h".into(),
            idempotency_key: "k".into(),
        };
        mailer.send_now(&email).await.unwrap();
        assert_eq!(mailer.outbox.as_ref().unwrap().to("a@x.org"), vec![email]);
    }

    #[test]
    fn smtp_url_is_validated() {
        assert!(SmtpSender::new("smtp://127.0.0.1:1025").is_ok());
        assert!(SmtpSender::new("http://127.0.0.1:1025").is_err());
    }
}
