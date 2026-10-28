//! Custom CA certificates (PLAN.md §3.3, Phase 4): loading, inspecting, and identifying
//! a set of extra roots so connections verified against different sets never share a
//! pool entry.

use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} contains no certificates (expected PEM or DER)")]
    Empty { path: String },
    #[error("{path} is not a valid certificate: {reason}")]
    Invalid { path: String, reason: String },
}

/// Loads every certificate in a PEM file (several allowed) or a single DER file.
pub fn load_ca_file(path: &Path) -> Result<Vec<CertificateDer<'static>>, CaError> {
    let shown = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|source| CaError::Read {
        path: shown.clone(),
        source,
    })?;
    let certs: Vec<CertificateDer<'static>> = if bytes.windows(11).any(|w| w == b"-----BEGIN ") {
        CertificateDer::pem_slice_iter(&bytes)
            .collect::<Result<_, _>>()
            .map_err(|e| CaError::Invalid {
                path: shown.clone(),
                reason: e.to_string(),
            })?
    } else {
        vec![CertificateDer::from(bytes)]
    };
    if certs.is_empty() {
        return Err(CaError::Empty { path: shown });
    }
    for cert in &certs {
        x509_parser::parse_x509_certificate(cert).map_err(|e| CaError::Invalid {
            path: shown.clone(),
            reason: e.to_string(),
        })?;
    }
    Ok(certs)
}

/// What the UI shows for a CA file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaInfo {
    /// Subject of the first certificate, e.g. `CN=Corp Root CA, O=Corp`.
    pub subject: Option<String>,
    /// Expiry of the first certificate, as Unix seconds.
    pub not_after: Option<i64>,
    /// Number of certificates in the file.
    pub count: u32,
}

pub fn inspect_ca_file(path: &Path) -> Result<CaInfo, CaError> {
    let certs = load_ca_file(path)?;
    let first = certs
        .first()
        .and_then(|c| x509_parser::parse_x509_certificate(c).ok())
        .map(|(_, cert)| (cert.subject().to_string(), cert.validity().not_after.timestamp()));
    Ok(CaInfo {
        subject: first.as_ref().map(|(s, _)| s.clone()),
        not_after: first.map(|(_, t)| t),
        count: certs.len() as u32,
    })
}

/// A set of extra trusted roots, identified by a SHA-256 over their DER bytes. Cheap to
/// clone.
#[derive(Clone)]
pub struct RootSet {
    certs: Arc<Vec<CertificateDer<'static>>>,
    id: [u8; 32],
}

impl RootSet {
    pub fn new(certs: Vec<CertificateDer<'static>>) -> Self {
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        for cert in &certs {
            ctx.update(&(cert.len() as u64).to_be_bytes());
            ctx.update(cert);
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(ctx.finish().as_ref());
        Self {
            certs: Arc::new(certs),
            id,
        }
    }

    /// Loads and combines several CA files.
    pub fn from_files<P: AsRef<Path>>(paths: &[P]) -> Result<Self, CaError> {
        let mut certs = Vec::new();
        for p in paths {
            certs.extend(load_ca_file(p.as_ref())?);
        }
        Ok(Self::new(certs))
    }

    pub fn certs(&self) -> &[CertificateDer<'static>] {
        &self.certs
    }

    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }

    pub(crate) fn id(&self) -> [u8; 32] {
        self.id
    }
}

impl std::fmt::Debug for RootSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootSet")
            .field("count", &self.certs.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ca_pem(cn: &str) -> String {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.distinguished_name.push(rcgen::DnType::CommonName, cn);
        params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap()
            .pem()
    }

    #[test]
    fn loads_and_inspects_multi_cert_pem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cas.pem");
        std::fs::write(&path, format!("{}{}", ca_pem("First CA"), ca_pem("Second CA"))).unwrap();
        assert_eq!(load_ca_file(&path).unwrap().len(), 2);
        let info = inspect_ca_file(&path).unwrap();
        assert_eq!(info.count, 2);
        assert_eq!(info.subject.as_deref(), Some("CN=First CA"));
        assert!(info.not_after.unwrap() > 0);
    }

    #[test]
    fn loads_der() {
        let dir = tempfile::tempdir().unwrap();
        let pem = ca_pem("Der CA");
        let der = CertificateDer::from_pem_slice(pem.as_bytes()).unwrap();
        let path = dir.path().join("ca.der");
        std::fs::write(&path, der.as_ref()).unwrap();
        assert_eq!(load_ca_file(&path).unwrap(), vec![der]);
    }

    #[test]
    fn rejects_garbage_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.pem");
        std::fs::write(&path, b"not a certificate").unwrap();
        assert!(matches!(load_ca_file(&path), Err(CaError::Invalid { .. })));
        assert!(matches!(
            load_ca_file(&dir.path().join("missing.pem")),
            Err(CaError::Read { .. })
        ));
    }

    #[test]
    fn root_set_identity_depends_on_contents() {
        let a = CertificateDer::from_pem_slice(ca_pem("A").as_bytes()).unwrap();
        let b = CertificateDer::from_pem_slice(ca_pem("B").as_bytes()).unwrap();
        assert_eq!(
            RootSet::new(vec![a.clone()]).id(),
            RootSet::new(vec![a.clone()]).id()
        );
        assert_ne!(RootSet::new(vec![a.clone()]).id(), RootSet::new(vec![b]).id());
        assert_ne!(RootSet::new(vec![a]).id(), RootSet::new(vec![]).id());
    }
}
