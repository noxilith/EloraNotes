//! TLS for the notes server.
//!
//! The server is reached by IP from other networks, so a CA-signed certificate is not an option.
//! Instead the server generates one self-signed certificate on first run, and the client pins
//! its SHA-256 fingerprint the first time it connects (shown as a short grouped code for the
//! person to eyeball). After that the connection fails unless the server presents the same
//! certificate, which is what stops anyone on the network from standing in the middle.

use crate::net::ServerConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::io::BufReader;
use std::sync::{Arc, Mutex};

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// SHA-256 of the DER certificate, lowercase hex.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert.as_ref());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Creates `cert.pem` and `key.pem` beside the config if they are not there yet.
pub fn ensure_identity(config: &ServerConfig) -> Result<(), String> {
    let cert_path = config.cert_path();
    let key_path = config.key_path();
    if cert_path.is_file() && key_path.is_file() {
        return Ok(());
    }

    let mut names = vec![
        config.name.clone(),
        "localhost".to_string(),
        String::from("127.0.0.1"),
        String::from("::1"),
    ];
    // Reachable by the machine's own addresses as well, so a client that resolves the server
    // by name or by IP both verify against the pinned fingerprint rather than the hostname.
    for ip in crate::net::local_ipv4_addresses() {
        names.push(ip.to_string());
    }
    let mut params = rcgen::CertificateParams::new(names)
        .map_err(|e| format!("could not prepare the server certificate: {e}"))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, config.name.clone());
    let key = rcgen::KeyPair::generate().map_err(|e| format!("could not generate a key: {e}"))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| format!("could not sign the certificate: {e}"))?;

    if let Some(dir) = cert_path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(&cert_path, cert.pem())
        .map_err(|e| format!("could not write {}: {e}", cert_path.display()))?;
    std::fs::write(&key_path, key.serialize_pem())
        .map_err(|e| format!("could not write {}: {e}", key_path.display()))?;
    Ok(())
}

/// The fingerprint of the certificate already on disk, for printing without starting a server.
pub fn read_fingerprint(config: &ServerConfig) -> Result<String, String> {
    let path = config.cert_path();
    let certs = read_certs(&path).map_err(|_| {
        format!(
            "{} has no certificate yet — start the server once to create it",
            path.display()
        )
    })?;
    let cert = certs
        .first()
        .ok_or_else(|| format!("{} holds no certificate", path.display()))?;
    Ok(fingerprint(cert))
}

fn read_certs(path: &std::path::Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{} is not a certificate: {e}", path.display()))
}

fn read_key(path: &std::path::Path) -> Result<PrivateKeyDer<'static>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    rustls_pemfile::private_key(&mut BufReader::new(file))
        .map_err(|e| format!("{} is not a private key: {e}", path.display()))?
        .ok_or_else(|| format!("{} holds no private key", path.display()))
}

/// The identity the server presents to clients.
pub fn server_config(config: &ServerConfig) -> Result<Arc<rustls::ServerConfig>, String> {
    ensure_identity(config)?;
    let certs = read_certs(&config.cert_path())?;
    let key = read_key(&config.key_path())?;
    let cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("the server certificate could not be used: {e}"))?;
    Ok(Arc::new(cfg))
}

/// Accepts the certificate, and remembers its fingerprint so the caller can show it or
/// compare it with what was pinned last time.
#[derive(Debug)]
struct PinnedVerifier {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = fingerprint(end_entity);
        if let Some(expected) = &self.expected {
            // A self-signed certificate has no chain to check, so the pinned fingerprint *is*
            // the verification.
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(rustls::Error::General(format!(
                    "the server's certificate fingerprint is {actual}, but {expected} was pinned"
                )));
            }
        }
        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some(actual);
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A client config that trusts `expected` (when given) and always reports the fingerprint the
/// server actually presented, so the UI can offer it for pinning on first connect.
pub fn client_config(
    expected: Option<String>,
) -> (Arc<rustls::ClientConfig>, Arc<Mutex<Option<String>>>) {
    let seen = Arc::new(Mutex::new(None));
    let provider = provider();
    let verifier = PinnedVerifier {
        expected: expected.filter(|f| !f.trim().is_empty()),
        seen: seen.clone(),
        provider: provider.clone(),
    };
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    (Arc::new(cfg), seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generated_identity_is_a_usable_certificate() {
        let dir = std::env::temp_dir().join(format!("elora-tls-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = ServerConfig {
            name: "Test".into(),
            port: 1,
            password: "pw".into(),
            notes_dir: dir.join("notes"),
            bind: "127.0.0.1".into(),
        };
        // The helpers write next to the config dir, so point ELORA_SERVER_DIR at the temp dir.
        // SAFETY: this test runs alone in its own process on this single-threaded path.
        unsafe { std::env::set_var("ELORA_SERVER_DIR", &dir) };
        ensure_identity(&config).expect("identity");
        assert!(config.cert_path().is_file());
        assert!(config.key_path().is_file());
        let certs = read_certs(&config.cert_path()).expect("cert");
        assert_eq!(fingerprint(&certs[0]).len(), 64);
        assert_eq!(read_fingerprint(&config).unwrap(), fingerprint(&certs[0]));
        assert!(server_config(&config).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
