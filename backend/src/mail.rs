//! Envoi d'e-mails (invitations, réinitialisation de mot de passe).
//!
//! `SmtpMailer` en production, `DisabledMailer` quand SMTP n'est pas configuré
//! (rien n'est envoyé, et surtout rien du contenu n'est journalisé : un lien de
//! réinitialisation dans les logs serait une faille), `MockMailer` en test.

use std::sync::Mutex;

use async_trait::async_trait;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub to: String,
    pub subject: String,
    /// Corps en texte brut.
    pub body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("adresse invalide : {0}")]
    InvalidAddress(String),
    #[error("envoi impossible : {0}")]
    Transport(String),
}

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, email: Email) -> Result<(), MailError>;

    /// Des e-mails partent réellement (SMTP configuré).
    fn is_enabled(&self) -> bool;
}

/// Chiffrement de la connexion SMTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    /// TLS dès la connexion (port 465).
    Tls,
    /// Connexion en clair puis STARTTLS obligatoire (port 587).
    StartTls,
    /// Aucun chiffrement : uniquement pour un relais local (Mailpit en dev).
    None,
}

pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Expéditeur, ex. `Lodestone <no-reply@mon-entreprise.fr>`.
    pub from: String,
}

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    pub fn new(config: &SmtpConfig) -> Result<Self, MailError> {
        let from: Mailbox = config
            .from
            .parse()
            .map_err(|_| MailError::InvalidAddress(config.from.clone()))?;
        let builder = match config.security {
            SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host),
            SmtpSecurity::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
            }
            SmtpSecurity::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &config.host,
            )),
        }
        .map_err(|e| MailError::Transport(e.to_string()))?;

        let mut builder = builder.port(config.port);
        if let (Some(user), Some(password)) = (&config.username, &config.password) {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        Ok(Self {
            transport: builder.build(),
            from,
        })
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, email: Email) -> Result<(), MailError> {
        let to: Mailbox = email
            .to
            .parse()
            .map_err(|_| MailError::InvalidAddress(email.to.clone()))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(email.subject)
            .header(ContentType::TEXT_PLAIN)
            .body(email.body)
            .map_err(|e| MailError::Transport(e.to_string()))?;
        self.transport
            .send(message)
            .await
            .map_err(|e| MailError::Transport(e.to_string()))?;
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// SMTP non configuré : n'envoie rien et ne journalise ni le destinataire ni
/// le contenu (qui peut contenir un lien de réinitialisation).
pub struct DisabledMailer;

#[async_trait]
impl Mailer for DisabledMailer {
    async fn send(&self, email: Email) -> Result<(), MailError> {
        tracing::warn!(subject = %email.subject, "e-mail NON envoyé : SMTP non configuré");
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        false
    }
}

/// Mailer de test : garde les e-mails « envoyés ».
#[derive(Default)]
pub struct MockMailer {
    sent: Mutex<Vec<Email>>,
}

impl MockMailer {
    pub fn sent(&self) -> Vec<Email> {
        self.sent.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl Mailer for MockMailer {
    async fn send(&self, email: Email) -> Result<(), MailError> {
        self.sent
            .lock()
            .map_err(|_| MailError::Transport("verrou empoisonné".into()))?
            .push(email);
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// Envoie en tâche de fond : la requête n'attend pas le serveur SMTP, et un
/// échec d'envoi est journalisé (sans le contenu) au lieu de faire échouer l'appel.
pub fn send_in_background(mailer: std::sync::Arc<dyn Mailer>, email: Email) {
    tokio::spawn(async move {
        let subject = email.subject.clone();
        if let Err(e) = mailer.send(email).await {
            tracing::error!(%subject, error = %e, "échec de l'envoi d'un e-mail");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(security: SmtpSecurity) -> SmtpConfig {
        SmtpConfig {
            host: "smtp.exemple.fr".into(),
            port: 587,
            security,
            username: Some("lodestone".into()),
            password: Some("secret".into()),
            from: "Lodestone <no-reply@exemple.fr>".into(),
        }
    }

    #[tokio::test]
    async fn construction_selon_le_chiffrement() {
        for security in [
            SmtpSecurity::Tls,
            SmtpSecurity::StartTls,
            SmtpSecurity::None,
        ] {
            assert!(SmtpMailer::new(&config(security)).is_ok(), "{security:?}");
        }
        let mut bad = config(SmtpSecurity::StartTls);
        bad.from = "pas une adresse".into();
        assert!(matches!(
            SmtpMailer::new(&bad),
            Err(MailError::InvalidAddress(_))
        ));
    }

    #[tokio::test]
    async fn mock_et_desactive() {
        let mock = MockMailer::default();
        let email = Email {
            to: "a@b.fr".into(),
            subject: "Sujet".into(),
            body: "Corps".into(),
        };
        mock.send(email.clone()).await.expect("envoi");
        assert_eq!(mock.sent(), vec![email.clone()]);
        assert!(mock.is_enabled());

        assert!(DisabledMailer.send(email).await.is_ok());
        assert!(!DisabledMailer.is_enabled());
    }
}
