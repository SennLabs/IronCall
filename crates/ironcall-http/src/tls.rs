//! TLS client configurations, one per (verify flag, ALPN preference), and a cache of
//! them per extra CA set.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::ca::RootSet;
use crate::engine::HttpVersionPref;

/// The engine's own configs plus one [`TlsConfigs`] per request-supplied [`RootSet`]
/// (engine roots + that set), built on first use.
pub(crate) struct TlsCache {
    engine_roots: Vec<CertificateDer<'static>>,
    base: TlsConfigs,
    extra: Mutex<HashMap<[u8; 32], Arc<TlsConfigs>>>,
}

impl TlsCache {
    pub(crate) fn new(engine_roots: Vec<CertificateDer<'static>>) -> Result<Self, rustls::Error> {
        Ok(Self {
            base: TlsConfigs::new(engine_roots.clone())?,
            engine_roots,
            extra: Mutex::default(),
        })
    }

    pub(crate) fn get(
        &self,
        verify: bool,
        pref: HttpVersionPref,
        roots: Option<&RootSet>,
    ) -> Result<Arc<ClientConfig>, rustls::Error> {
        let Some(roots) = roots.filter(|r| verify && !r.is_empty()) else {
            return Ok(self.base.get(verify, pref));
        };
        let mut extra = self.extra.lock().expect("TLS cache lock poisoned");
        let configs = match extra.get(&roots.id()) {
            Some(c) => c.clone(),
            None => {
                let mut all = self.engine_roots.clone();
                all.extend(roots.certs().iter().cloned());
                let built = Arc::new(TlsConfigs::new(all)?);
                extra.insert(roots.id(), built.clone());
                built
            }
        };
        Ok(configs.get(verify, pref))
    }
}

/// Cached client configs. Extra roots are fixed at construction (PLAN.md §3.3), so a
/// different CA set means a different `TlsConfigs` (and a different pool key).
#[derive(Debug)]
pub(crate) struct TlsConfigs {
    verified: [Arc<ClientConfig>; 3],
    unverified: [Arc<ClientConfig>; 3],
}

impl TlsConfigs {
    pub(crate) fn new(extra_roots: Vec<CertificateDer<'static>>) -> Result<Self, rustls::Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier: Arc<dyn ServerCertVerifier> = Arc::new(
            rustls_platform_verifier::Verifier::new_with_extra_roots(extra_roots, provider.clone())?,
        );
        let no_verify: Arc<dyn ServerCertVerifier> = Arc::new(NoVerify(provider.clone()));

        let build = |verifier: &Arc<dyn ServerCertVerifier>, pref: HttpVersionPref| {
            let mut config = ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()?
                .dangerous()
                .with_custom_certificate_verifier(verifier.clone())
                .with_no_client_auth();
            config.alpn_protocols = match pref {
                HttpVersionPref::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
                HttpVersionPref::Http1 => vec![b"http/1.1".to_vec()],
                HttpVersionPref::Http2 => vec![b"h2".to_vec()],
            };
            Ok::<_, rustls::Error>(Arc::new(config))
        };
        let all = |v: &Arc<dyn ServerCertVerifier>| -> Result<[Arc<ClientConfig>; 3], rustls::Error> {
            Ok([
                build(v, HttpVersionPref::Auto)?,
                build(v, HttpVersionPref::Http1)?,
                build(v, HttpVersionPref::Http2)?,
            ])
        };
        Ok(Self {
            verified: all(&verifier)?,
            unverified: all(&no_verify)?,
        })
    }

    pub(crate) fn get(&self, verify: bool, pref: HttpVersionPref) -> Arc<ClientConfig> {
        let set = if verify { &self.verified } else { &self.unverified };
        let i = match pref {
            HttpVersionPref::Auto => 0,
            HttpVersionPref::Http1 => 1,
            HttpVersionPref::Http2 => 2,
        };
        set[i].clone()
    }
}

/// Accepts any server certificate but still checks handshake signatures, so the peer
/// must hold the private key for the certificate it presents (PLAN.md §3.3).
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
