//! Device keys: a phone proves who it is with a P-256 key pair instead of only
//! claiming a device id (docs/PROTOCOL.md, "Device keys"). One implementation
//! for both sides: the app builds its hello fields and the payload to sign
//! here (through the FFI), the hub verifies here.
//!
//! - The public key travels as SEC1 uncompressed (65 bytes, the X9.63 form
//!   CryptoKit calls `x963Representation`), standard base64.
//! - The device id is [`PublicKey::device_id`]: the first 16 bytes of the
//!   SHA-256 of that encoding, as 32 lowercase hex characters. A hello whose
//!   id is not derived from its key is refused, so an id can not be claimed
//!   without the key.
//! - The server answers a keyed hello with a `challenge` (`server`, `nonce`);
//!   the device signs [`auth_payload`], which includes the host it
//!   connected to, and answers with `auth`. Signatures
//!   are ECDSA P-256 with SHA-256, `r || s` (64 bytes), standard base64.
//!
//! The private key never passes through here on a phone: the app keeps it in
//! the Secure Enclave and signs the bytes this module hands it.
//! [`SoftwareKey`] signs like the phone does, for test devices and tests.
//!
//! No randomness here (the pure core builds for wasm without an entropy
//! source); servers draw nonces and test keys themselves.

use std::fmt;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::protocol::DeviceMessage;

/// Length of a device id in hex characters (16 bytes of the fingerprint).
pub const DEVICE_ID_HEX_LEN: usize = 32;

/// Domain separation tag at the start of every signed `auth` payload. The
/// version changes when the payload does (v2 added the host).
pub const AUTH_CONTEXT: &str = "tallycam-lcp-auth-v2";

/// Longest `server` and `nonce` a device signs, in bytes.
pub const MAX_CHALLENGE_FIELD: usize = 128;

/// Why a key, challenge or signature was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Not base64, or not a point on P-256.
    BadPublicKey,
    /// Not base64, or not 64 bytes, or not a valid signature encoding.
    BadSignatureEncoding,
    /// Well formed, but does not verify with the key.
    SignatureMismatch,
    /// A challenge field is empty, too long, or not printable ASCII.
    BadChallenge,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeyError::BadPublicKey => "not a P-256 public key",
            KeyError::BadSignatureEncoding => "not a P-256 signature (64 bytes r || s)",
            KeyError::SignatureMismatch => "the signature does not verify with the device key",
            KeyError::BadChallenge => "not a usable challenge",
        })
    }
}

impl std::error::Error for KeyError {}

/// Printable ASCII without spaces, 1 to [`MAX_CHALLENGE_FIELD`] bytes: no
/// field can carry the payload's `\n` separator.
fn usable_field(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_CHALLENGE_FIELD && s.bytes().all(|b| b.is_ascii_graphic())
}

/// The bytes a device signs to answer a `challenge`: the tag, the server id
/// and nonce from the challenge, the device id and the host it connected
/// to ([`auth_host`]), joined by `\n`. The nonce ties the signature to that
/// one connection, so it can not be replayed. The host is what the device
/// itself dialled: a server it was led to can not pass the signature on to
/// another server, which got the connection under its own host. Refuses
/// fields that could smuggle a separator.
pub fn auth_payload(
    server: &str,
    nonce: &str,
    device_id: &str,
    host: &str,
) -> Result<Vec<u8>, KeyError> {
    if ![server, nonce, device_id, host]
        .iter()
        .all(|f| usable_field(f))
    {
        return Err(KeyError::BadChallenge);
    }
    Ok(format!("{AUTH_CONTEXT}\n{server}\n{nonce}\n{device_id}\n{host}").into_bytes())
}

/// The `auth` message for a signature (`r || s`, 64 bytes, as CryptoKit's
/// `rawRepresentation`) made for `host`, as the JSON text to send.
pub fn auth_message(signature: &[u8], host: &str) -> Result<String, KeyError> {
    Signature::from_slice(signature).map_err(|_| KeyError::BadSignatureEncoding)?;
    let msg = DeviceMessage::Auth {
        signature: BASE64.encode(signature),
        host: host.to_string(),
    };
    serde_json::to_string(&msg).map_err(|_| KeyError::BadSignatureEncoding)
}

/// The host a device signs for its control URL (`ws://` or `wss://`):
/// the authority, lowercase, without user info and without the scheme's
/// default port (`hub.example`, `192.0.2.10:8090`, `[2001:db8::1]:8090`).
/// None for another scheme or no host.
pub fn auth_host(control_url: &str) -> Option<String> {
    let (scheme, rest) = control_url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ws" | "http" => ":80",
        "wss" | "https" => ":443",
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?.to_ascii_lowercase();
    let host = authority.strip_suffix(default_port).unwrap_or(&authority);
    let bare = host
        .strip_prefix('[')
        .map_or(host, |h| h.split(']').next().unwrap_or(""));
    if bare.is_empty() || bare.starts_with(':') || !usable_field(host) {
        return None;
    }
    Some(host.to_string())
}

/// A server's view of the host a device reached it by (the `Host` header,
/// or a proxy's `X-Forwarded-Host`), in [`auth_host`]'s form: lowercase,
/// and without a port of 80 or 443 (the header has no scheme).
pub fn normalize_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    for port in [":80", ":443"] {
        if let Some(h) = host.strip_suffix(port) {
            return h.to_string();
        }
    }
    host
}

/// A device's public key, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    key: VerifyingKey,
    /// SEC1 uncompressed, the canonical form ids are derived from.
    sec1: Vec<u8>,
}

impl PublicKey {
    /// From SEC1 bytes (uncompressed, as X9.63, or compressed).
    pub fn from_sec1(bytes: &[u8]) -> Result<Self, KeyError> {
        let key = VerifyingKey::from_sec1_bytes(bytes).map_err(|_| KeyError::BadPublicKey)?;
        Ok(Self::from_verifying_key(key))
    }

    /// From base64 SEC1, as in `hello.public_key`.
    pub fn from_base64(text: &str) -> Result<Self, KeyError> {
        let bytes = BASE64
            .decode(text.trim())
            .map_err(|_| KeyError::BadPublicKey)?;
        Self::from_sec1(&bytes)
    }

    fn from_verifying_key(key: VerifyingKey) -> Self {
        let sec1 = key.to_encoded_point(false).as_bytes().to_vec();
        Self { key, sec1 }
    }

    /// SEC1 uncompressed bytes (X9.63).
    pub fn to_sec1(&self) -> Vec<u8> {
        self.sec1.clone()
    }

    /// Canonical base64 form (SEC1 uncompressed): `hello.public_key`, and
    /// what servers store.
    pub fn to_base64(&self) -> String {
        BASE64.encode(&self.sec1)
    }

    /// SHA-256 of the SEC1 uncompressed key, 64 lowercase hex characters.
    pub fn fingerprint(&self) -> String {
        hex(&Sha256::digest(&self.sec1))
    }

    /// The device id this key stands for: the first 32 hex characters of
    /// [`Self::fingerprint`].
    pub fn device_id(&self) -> String {
        let mut id = self.fingerprint();
        id.truncate(DEVICE_ID_HEX_LEN);
        id
    }

    /// Checks `signature` (base64 `r || s`) over `message`.
    pub fn verify(&self, message: &[u8], signature: &str) -> Result<(), KeyError> {
        let bytes = BASE64
            .decode(signature.trim())
            .map_err(|_| KeyError::BadSignatureEncoding)?;
        self.verify_raw(message, &bytes)
    }

    /// Checks a raw `r || s` signature over `message`.
    pub fn verify_raw(&self, message: &[u8], signature: &[u8]) -> Result<(), KeyError> {
        let sig = Signature::from_slice(signature).map_err(|_| KeyError::BadSignatureEncoding)?;
        // CryptoKit does not normalize s; accept both halves like it does.
        let sig = sig.normalize_s().unwrap_or(sig);
        self.key
            .verify(message, &sig)
            .map_err(|_| KeyError::SignatureMismatch)
    }

    /// Checks an `auth` answer (base64 signature) to the challenge
    /// (`server`, `nonce`) from the device `device_id`, signed for `host`.
    /// The caller checks that `host` is its own.
    pub fn verify_auth(
        &self,
        server: &str,
        nonce: &str,
        device_id: &str,
        host: &str,
        signature: &str,
    ) -> Result<(), KeyError> {
        self.verify(&auth_payload(server, nonce, device_id, host)?, signature)
    }
}

/// The device id for a base64 public key, or `None` when it is not a key.
pub fn device_id(public_key: &str) -> Option<String> {
    PublicKey::from_base64(public_key)
        .ok()
        .map(|k| k.device_id())
}

/// The fingerprint for a base64 public key, or `None` when it is not a key.
pub fn fingerprint(public_key: &str) -> Option<String> {
    PublicKey::from_base64(public_key)
        .ok()
        .map(|k| k.fingerprint())
}

/// A P-256 signing key in memory, for test devices. Persisted as the 32-byte
/// secret scalar in hex.
pub struct SoftwareKey {
    key: SigningKey,
}

impl fmt::Debug for SoftwareKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoftwareKey")
            .field("device_id", &self.public_key().device_id())
            .finish_non_exhaustive()
    }
}

impl SoftwareKey {
    /// The key for a 32-byte secret; `None` for zero or a value above the
    /// curve order (draw again).
    pub fn from_secret(secret: &[u8; 32]) -> Option<Self> {
        SigningKey::from_slice(secret).ok().map(|key| Self { key })
    }

    /// The key from its hex secret.
    pub fn from_hex(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.len() != 64 || !text.is_ascii() {
            return Err("expected 64 hex characters".into());
        }
        let mut secret = [0u8; 32];
        for (i, b) in secret.iter_mut().enumerate() {
            *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
        }
        Self::from_secret(&secret).ok_or_else(|| "not a P-256 secret".to_string())
    }

    /// The secret scalar in hex.
    pub fn to_hex(&self) -> String {
        hex(&self.key.to_bytes())
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_verifying_key(*self.key.verifying_key())
    }

    /// Signs `message`: raw `r || s`.
    pub fn sign_raw(&self, message: &[u8]) -> [u8; 64] {
        let sig: Signature = self.key.sign(message);
        sig.to_bytes().into()
    }

    /// Signs `message`: base64 `r || s`.
    pub fn sign(&self, message: &[u8]) -> String {
        BASE64.encode(self.sign_raw(message))
    }

    /// The base64 `auth` signature for a challenge, for `host` (what the
    /// device connected to, [`auth_host`]); empty for an unusable challenge
    /// (a server would refuse the answer anyway).
    pub fn sign_auth(&self, server: &str, nonce: &str, host: &str) -> String {
        match auth_payload(server, nonce, &self.public_key().device_id(), host) {
            Ok(payload) => self.sign(&payload),
            Err(_) => String::new(),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SoftwareKey {
        SoftwareKey::from_secret(&[seed; 32]).unwrap()
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let key = key(7);
        let public = key.public_key();
        let id = public.device_id();
        assert_eq!(id.len(), DEVICE_ID_HEX_LEN);
        assert!(public.fingerprint().starts_with(&id));
        let b64 = public.to_base64();
        assert_eq!(BASE64.decode(&b64).unwrap().len(), 65);
        assert_eq!(device_id(&b64).as_deref(), Some(id.as_str()));

        let sig = key.sign_auth("srv", "nonce1", "hub");
        let parsed = PublicKey::from_base64(&b64).unwrap();
        assert_eq!(
            parsed.verify_auth("srv", "nonce1", &id, "hub", &sig),
            Ok(())
        );
        assert_eq!(
            parsed.verify_auth("srv", "nonce2", &id, "hub", &sig),
            Err(KeyError::SignatureMismatch)
        );
        assert_eq!(
            parsed.verify_auth("other", "nonce1", &id, "hub", &sig),
            Err(KeyError::SignatureMismatch)
        );
        // Signed for another host: relayed by a server it was led to.
        assert_eq!(
            parsed.verify_auth("srv", "nonce1", &id, "evil", &sig),
            Err(KeyError::SignatureMismatch)
        );
        assert_eq!(
            parsed.verify_auth("srv", "nonce1", &id, "hub", "bm90IGEgc2ln"),
            Err(KeyError::BadSignatureEncoding)
        );
        let other = self::key(8).public_key();
        assert_eq!(
            other.verify_auth("srv", "nonce1", &id, "hub", &sig),
            Err(KeyError::SignatureMismatch)
        );
    }

    #[test]
    fn payload_refuses_fields_that_could_carry_a_separator() {
        assert_eq!(
            auth_payload("s", "n", "d", "h").unwrap(),
            b"tallycam-lcp-auth-v2\ns\nn\nd\nh".to_vec()
        );
        assert_eq!(
            auth_payload("s\nx", "n", "d", "h"),
            Err(KeyError::BadChallenge)
        );
        assert_eq!(auth_payload("s", "", "d", "h"), Err(KeyError::BadChallenge));
        assert_eq!(
            auth_payload("s", "n", "a b", "h"),
            Err(KeyError::BadChallenge)
        );
        assert_eq!(
            auth_payload("s", "n", "d", "h\nx"),
            Err(KeyError::BadChallenge)
        );
        let long = "x".repeat(MAX_CHALLENGE_FIELD + 1);
        assert_eq!(
            auth_payload(&long, "n", "d", "h"),
            Err(KeyError::BadChallenge)
        );
    }

    #[test]
    fn the_signed_host_is_the_authority_as_the_server_sees_it() {
        for (url, host) in [
            (
                "wss://Hub.Example/lcp/v0/device?token=x",
                Some("hub.example"),
            ),
            ("wss://hub.example:443/x", Some("hub.example")),
            ("ws://hub.example:80", Some("hub.example")),
            (
                "ws://192.0.2.10:8090/lcp/v0/device",
                Some("192.0.2.10:8090"),
            ),
            ("wss://hub.example:8443/", Some("hub.example:8443")),
            ("ws://[2001:db8::1]:8090/d", Some("[2001:db8::1]:8090")),
            ("ws://user@hub.example/d", Some("hub.example")),
            ("ws://", None),
            ("ws://:8090/d", None),
            ("srt://hub.example:9000", None),
            ("hub.example", None),
        ] {
            assert_eq!(auth_host(url).as_deref(), host, "{url}");
        }
        assert_eq!(normalize_host("Hub.Example:443"), "hub.example");
        assert_eq!(normalize_host("hub.example:80"), "hub.example");
        assert_eq!(normalize_host("192.0.2.10:8090"), "192.0.2.10:8090");
        assert_eq!(normalize_host("[2001:db8::1]:443"), "[2001:db8::1]");
    }

    #[test]
    fn auth_message_carries_the_signature() {
        let key = key(9);
        let raw = key.sign_raw(b"x");
        let text = auth_message(&raw, "hub.example").unwrap();
        let msg: DeviceMessage = serde_json::from_str(&text).unwrap();
        let DeviceMessage::Auth { signature, host } = msg else {
            panic!("not auth: {text}")
        };
        assert_eq!(host, "hub.example");
        assert_eq!(key.public_key().verify(b"x", &signature), Ok(()));
        assert_eq!(
            auth_message(&[0; 10], "h"),
            Err(KeyError::BadSignatureEncoding)
        );
    }

    #[test]
    fn hex_secret_round_trips_and_bad_keys_are_refused() {
        let key = key(3);
        let again = SoftwareKey::from_hex(&key.to_hex()).unwrap();
        assert_eq!(again.public_key(), key.public_key());
        assert!(SoftwareKey::from_hex("00").is_err());
        assert!(SoftwareKey::from_secret(&[0; 32]).is_none());
        assert_eq!(PublicKey::from_base64("BAE="), Err(KeyError::BadPublicKey));
        assert_eq!(device_id("%%%"), None);
    }

    /// A key and signature made by CryptoKit on macOS
    /// (`P256.Signing.PrivateKey`), so both ends agree on the encodings, the
    /// id derivation and the payload.
    #[test]
    fn verifies_a_cryptokit_signature() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/vectors/cryptokit_auth.json")).unwrap();
        let s = |k: &str| v[k].as_str().unwrap().to_string();
        let key = PublicKey::from_base64(&s("public_key")).unwrap();
        assert_eq!(key.device_id(), s("device_id"));
        assert_eq!(
            key.verify_auth(
                &s("server"),
                &s("nonce"),
                &s("device_id"),
                &s("host"),
                &s("signature")
            ),
            Ok(())
        );
    }
}
