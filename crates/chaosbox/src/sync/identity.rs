//! User-root enrollment and device signatures. Secret keys never travel to peers.
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{self, Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};

/// Root-authorized membership in one user's private visibility scopes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    /// User-root Ed25519 public key, hex encoded.
    pub user: String,
    /// Device Ed25519 public key, hex encoded.
    pub device: String,
    /// Explicit scopes this device may replicate.
    pub scopes: Vec<String>,
    /// Root signature, domain separated from event signatures.
    pub signature: String,
}

/// Offline enrollment authority. Keep its private key on the enrollment host.
pub struct Authority(Vec<u8>);

/// A device's private key and public membership grant.
pub struct Identity {
    key: Ed25519KeyPair,
    /// Public root-authorized membership.
    pub grant: Grant,
}

fn key(bytes: &[u8]) -> Result<Ed25519KeyPair, String> {
    Ed25519KeyPair::from_pkcs8(bytes).map_err(|_| "invalid Ed25519 private key".into())
}

/// Generate a PKCS#8 private key using the operating system random source.
pub fn generate_key() -> Result<Vec<u8>, String> {
    Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map(|k| k.as_ref().to_vec())
        .map_err(|_| "key generation failed".into())
}

/// A random session challenge; it contains no credential material.
pub fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "random challenge failed")?;
    Ok(hex::encode(bytes))
}

/// Hex encoding of a private key's public identity.
pub fn public_key(bytes: &[u8]) -> Result<String, String> {
    Ok(hex::encode(key(bytes)?.public_key().as_ref()))
}

/// Verify a domain-separated Ed25519 signature.
pub fn verify(public: &str, domain: &str, bytes: &[u8], signed: &str) -> Result<(), String> {
    let public = hex::decode(public).map_err(|_| "invalid public key")?;
    let signed = hex::decode(signed).map_err(|_| "invalid signature")?;
    if public.len() != 32 || signed.len() != 64 {
        return Err("invalid signing identity".into());
    }
    let mut data = domain.as_bytes().to_vec();
    data.extend_from_slice(bytes);
    signature::UnparsedPublicKey::new(&signature::ED25519, public)
        .verify(&data, &signed)
        .map_err(|_| "signature verification failed".into())
}

fn sign(key: &Ed25519KeyPair, domain: &str, bytes: &[u8]) -> String {
    let mut data = domain.as_bytes().to_vec();
    data.extend_from_slice(bytes);
    hex::encode(key.sign(&data).as_ref())
}

impl Authority {
    /// Load the operator-owned enrollment key.
    pub fn from_pkcs8(bytes: Vec<u8>) -> Result<Self, String> {
        key(&bytes)?;
        Ok(Self(bytes))
    }
    /// Private bytes for a new, mode-0600 enrollment file.
    #[must_use]
    pub fn private_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Grant a device explicit private scopes.
    pub fn authorize(&self, device: &str, mut scopes: Vec<String>) -> Result<Grant, String> {
        if hex::decode(device).map_err(|_| "invalid device key")?.len() != 32
            || scopes.is_empty()
            || scopes
                .iter()
                .any(|s| !s.starts_with("private:") || s.len() <= 8)
        {
            return Err("enrollment requires a device key and private scopes".into());
        }
        scopes.sort();
        scopes.dedup();
        let root = key(&self.0)?;
        let user = hex::encode(root.public_key().as_ref());
        let data = serde_json::to_vec(&(&user, device, &scopes)).map_err(|_| "encode grant")?;
        Ok(Grant {
            user,
            device: device.into(),
            scopes,
            signature: sign(&root, "chaosbox-grant-v1\0", &data),
        })
    }
}

impl Grant {
    /// Check root membership and exact scope authorization before disclosure.
    pub fn validate(&self, user: &str, scope: &str) -> Result<(), String> {
        if self.user != user
            || !self.scopes.iter().any(|s| s == scope)
            || self.scopes.windows(2).any(|w| w[0] >= w[1])
        {
            return Err("device is outside the authorized user or scope".into());
        }
        let data = serde_json::to_vec(&(&self.user, &self.device, &self.scopes))
            .map_err(|_| "encode grant")?;
        verify(user, "chaosbox-grant-v1\0", &data, &self.signature)
    }
}

impl Identity {
    /// Create a new user and its first enrolled device.
    pub fn create(scope: &str) -> Result<(Self, Authority), String> {
        let authority = Authority::from_pkcs8(generate_key()?)?;
        Ok((Self::enrolled(&authority, scope)?, authority))
    }
    /// Enroll a fresh device with an available offline authority.
    pub fn enrolled(authority: &Authority, scope: &str) -> Result<Self, String> {
        let private = generate_key()?;
        let grant = authority.authorize(&public_key(&private)?, vec![scope.into()])?;
        Self::from_pkcs8(&private, grant, scope)
    }
    /// Load a device only when its private key matches the enrolled key.
    pub fn from_pkcs8(bytes: &[u8], grant: Grant, scope: &str) -> Result<Self, String> {
        grant.validate(&grant.user, scope)?;
        let key = key(bytes)?;
        if hex::encode(key.public_key().as_ref()) != grant.device {
            return Err("grant/private key mismatch".into());
        }
        Ok(Self { key, grant })
    }
    /// The stable user identity, independent of Unix names and UIDs.
    #[must_use]
    pub fn user(&self) -> String {
        self.grant.user.clone()
    }
    /// Sign a typed public object for events or session messages.
    #[must_use]
    pub fn sign(&self, domain: &str, bytes: &[u8]) -> String {
        sign(&self.key, domain, bytes)
    }
}
