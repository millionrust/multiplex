use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::Certificate;
use russh::keys::key::PrivateKeyWithHashAlg;

use crate::credentials;
use crate::models::AuthConfig;

const MAX_OPENSSH_CERTIFICATE_BYTES: u64 = 64 * 1024;

pub async fn authenticate<H: client::Handler>(
    handle: &mut client::Handle<H>,
    username: &str,
    auth: &AuthConfig,
) -> Result<()> {
    let auth_result = match auth {
        AuthConfig::Password { password } => handle
            .authenticate_password(username.to_string(), password.clone())
            .await
            .context("Password authentication failed")?,
        AuthConfig::PasswordRef { credential_id } => {
            let password = credentials::load_password(credential_id)
                .context("Unable to load the saved SSH password")?;
            handle
                .authenticate_password(username.to_string(), password)
                .await
                .context("Password authentication failed")?
        }
        AuthConfig::PrivateKey {
            key_path,
            passphrase,
        } => {
            let key = load_private_key(key_path, passphrase.as_deref())?;
            let rsa_hash = handle
                .best_supported_rsa_hash()
                .await
                .context("Unable to negotiate an RSA signature algorithm")?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);
            handle
                .authenticate_publickey(username.to_string(), key)
                .await
                .context("Public key authentication failed")?
        }
        AuthConfig::OpenSshCertificate {
            key_path,
            certificate_path,
            passphrase,
        } => {
            let key = load_private_key(key_path, passphrase.as_deref())?;
            let certificate = load_user_certificate(certificate_path, username, &key)?;
            handle
                .authenticate_openssh_cert(username.to_string(), Arc::new(key), certificate)
                .await
                .context("OpenSSH certificate authentication failed")?
        }
    };

    if !auth_result.success() {
        bail!("Authentication was rejected by the server");
    }
    Ok(())
}

fn load_private_key(key_path: &str, passphrase: Option<&str>) -> Result<russh::keys::PrivateKey> {
    russh::keys::load_secret_key(key_path, passphrase).map_err(|error| {
        let message = error.to_string();
        if message.to_ascii_lowercase().contains("encrypt") && passphrase.is_none() {
            anyhow::anyhow!(
                "The configured private key is passphrase-protected. Enter its passphrase and try again."
            )
        } else {
            anyhow::anyhow!("Unable to load the configured private key")
        }
    })
}

fn load_user_certificate(
    certificate_path: &str,
    username: &str,
    private_key: &russh::keys::PrivateKey,
) -> Result<Certificate> {
    let certificate = read_current_user_certificate(Path::new(certificate_path))?;
    if certificate.public_key() != private_key.public_key().key_data() {
        bail!("The configured OpenSSH certificate does not match the private key");
    }
    let principals = certificate.valid_principals();
    if !principals.is_empty() && !principals.iter().any(|principal| principal == username) {
        bail!("The configured OpenSSH certificate does not authorize this username");
    }

    Ok(certificate)
}

pub fn inspect_user_certificate_file(path: &Path) -> Result<()> {
    read_current_user_certificate(path).map(|_| ())
}

fn read_current_user_certificate(path: &Path) -> Result<Certificate> {
    let metadata =
        std::fs::metadata(path).context("Unable to inspect the configured OpenSSH certificate")?;
    if !metadata.is_file() {
        bail!("The configured OpenSSH certificate is not a regular file");
    }
    if metadata.len() > MAX_OPENSSH_CERTIFICATE_BYTES {
        bail!("The configured OpenSSH certificate exceeds the 64 KiB limit");
    }
    let encoded = std::fs::read_to_string(path)
        .context("Unable to read the configured OpenSSH certificate")?;
    let certificate = Certificate::from_openssh(&encoded)
        .context("The configured OpenSSH certificate is malformed")?;

    if !certificate.cert_type().is_user() {
        bail!("The configured OpenSSH certificate is not a user certificate");
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("The system clock is before the Unix epoch")?
        .as_secs();
    if now < certificate.valid_after() {
        bail!("The configured OpenSSH certificate is not valid yet");
    }
    if now >= certificate.valid_before() {
        bail!("The configured OpenSSH certificate has expired");
    }
    Ok(certificate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::certificate::{Builder, CertType};
    use tempfile::TempDir;

    fn test_key() -> russh::keys::PrivateKey {
        russh::keys::load_secret_key("tests/fixtures/ssh-server/id_ed25519", None).unwrap()
    }

    fn write_certificate(
        directory: &TempDir,
        subject: &russh::keys::PrivateKey,
        signer: &russh::keys::PrivateKey,
        cert_type: CertType,
        valid_after: u64,
        valid_before: u64,
        principals: &[&str],
    ) -> String {
        let mut builder = Builder::new_with_random_nonce(
            &mut rand::rngs::OsRng,
            subject.public_key().key_data().clone(),
            valid_after,
            valid_before,
        )
        .unwrap();
        builder.cert_type(cert_type).unwrap();
        builder.key_id("termirust-test").unwrap();
        for principal in principals {
            builder.valid_principal((*principal).to_string()).unwrap();
        }
        let certificate = builder.sign(signer).unwrap();
        let path = directory.path().join("identity-cert.pub");
        certificate.write_file(&path).unwrap();
        path.display().to_string()
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn certificate_validation_accepts_matching_current_user_certificate() {
        let directory = TempDir::new().unwrap();
        let key = test_key();
        let current = now();
        let path = write_certificate(
            &directory,
            &key,
            &key,
            CertType::User,
            current - 60,
            current + 60,
            &["termirust"],
        );
        load_user_certificate(&path, "termirust", &key).unwrap();
    }

    #[test]
    fn certificate_validation_rejects_wrong_type_time_principal_and_key() {
        let directory = TempDir::new().unwrap();
        let key = test_key();
        let other = russh::keys::PrivateKey::random(
            &mut rand::rngs::OsRng,
            russh::keys::Algorithm::Ed25519,
        )
        .unwrap();
        let current = now();

        let host = write_certificate(
            &directory,
            &key,
            &key,
            CertType::Host,
            current - 60,
            current + 60,
            &["termirust"],
        );
        assert!(load_user_certificate(&host, "termirust", &key).is_err());

        let future = write_certificate(
            &directory,
            &key,
            &key,
            CertType::User,
            current + 60,
            current + 120,
            &["termirust"],
        );
        assert!(load_user_certificate(&future, "termirust", &key).is_err());

        let expired = write_certificate(
            &directory,
            &key,
            &key,
            CertType::User,
            current - 120,
            current - 60,
            &["termirust"],
        );
        assert!(load_user_certificate(&expired, "termirust", &key).is_err());

        let wrong_principal = write_certificate(
            &directory,
            &key,
            &key,
            CertType::User,
            current - 60,
            current + 60,
            &["another-user"],
        );
        assert!(load_user_certificate(&wrong_principal, "termirust", &key).is_err());

        let wrong_key = write_certificate(
            &directory,
            &other,
            &other,
            CertType::User,
            current - 60,
            current + 60,
            &["termirust"],
        );
        assert!(load_user_certificate(&wrong_key, "termirust", &key).is_err());
    }

    #[test]
    fn certificate_file_validation_rejects_missing_malformed_and_oversized_files() {
        let directory = TempDir::new().unwrap();
        let missing = directory.path().join("missing-cert.pub");
        assert!(inspect_user_certificate_file(&missing).is_err());

        let malformed = directory.path().join("malformed-cert.pub");
        std::fs::write(&malformed, "not an OpenSSH certificate").unwrap();
        assert!(inspect_user_certificate_file(&malformed).is_err());

        let oversized = directory.path().join("oversized-cert.pub");
        std::fs::write(
            &oversized,
            vec![b'x'; MAX_OPENSSH_CERTIFICATE_BYTES as usize + 1],
        )
        .unwrap();
        let error = inspect_user_certificate_file(&oversized).unwrap_err();
        assert!(error.to_string().contains("64 KiB limit"));
    }

    #[test]
    fn private_key_load_errors_do_not_expose_paths_or_passphrases() {
        let key_path = "/private/customer/acme/id_ed25519";
        let passphrase = "customer-secret-passphrase";
        let error = load_private_key(key_path, Some(passphrase)).unwrap_err();
        let message = error.to_string();
        assert!(!message.contains(key_path));
        assert!(!message.contains(passphrase));
        assert_eq!(message, "Unable to load the configured private key");
    }
}
