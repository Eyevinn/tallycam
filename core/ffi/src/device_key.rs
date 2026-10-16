//! Device keys for the app: everything but the key itself. The app creates
//! the key in the Secure Enclave (a software key on the simulator), hands
//! the public key here for its id and hello field, signs the payload this
//! module builds for a `challenge`, and sends the `auth` message built here.
//! See `tallycam_core::device_key`.

use tallycam_core::device_key::{self as core_key, KeyError, PublicKey};

#[derive(Debug, uniffi::Error)]
pub enum DeviceKeyError {
    /// The public key is not a P-256 point.
    PublicKey,
    /// The challenge is not usable (empty, too long or not printable).
    Challenge,
    /// The signature is not 64 bytes `r || s`, or does not verify.
    Signature,
}

impl std::fmt::Display for DeviceKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DeviceKeyError::PublicKey => "not a P-256 public key",
            DeviceKeyError::Challenge => "not a usable challenge",
            DeviceKeyError::Signature => "not a valid signature",
        })
    }
}

impl std::error::Error for DeviceKeyError {}

impl From<KeyError> for DeviceKeyError {
    fn from(e: KeyError) -> Self {
        match e {
            KeyError::BadPublicKey => DeviceKeyError::PublicKey,
            KeyError::BadChallenge => DeviceKeyError::Challenge,
            KeyError::BadSignatureEncoding | KeyError::SignatureMismatch => {
                DeviceKeyError::Signature
            }
        }
    }
}

/// Who a public key makes this device.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DeviceKeyIdentity {
    /// `hello.public_key`: base64 SEC1 uncompressed.
    pub public_key: String,
    /// `hello.device.id`.
    pub device_id: String,
    /// SHA-256 of the key, hex, to show next to what the server shows.
    pub fingerprint: String,
}

/// The identity for a public key (CryptoKit `x963Representation`).
#[uniffi::export]
pub fn device_key_identity(public_key_x963: Vec<u8>) -> Result<DeviceKeyIdentity, DeviceKeyError> {
    let key = PublicKey::from_sec1(&public_key_x963)?;
    Ok(DeviceKeyIdentity {
        public_key: key.to_base64(),
        device_id: key.device_id(),
        fingerprint: key.fingerprint(),
    })
}

/// The bytes to sign for a `challenge` (its `server` and `nonce`).
#[uniffi::export]
pub fn device_key_auth_payload(
    server: String,
    nonce: String,
    device_id: String,
) -> Result<Vec<u8>, DeviceKeyError> {
    Ok(core_key::auth_payload(&server, &nonce, &device_id)?)
}

/// The `auth` message (JSON text) for a raw `r || s` signature (CryptoKit
/// `rawRepresentation`).
#[uniffi::export]
pub fn device_key_auth_message(signature: Vec<u8>) -> Result<String, DeviceKeyError> {
    Ok(core_key::auth_message(&signature)?)
}

/// Does `signature` (raw `r || s`) by `public_key_x963` verify over
/// `payload`? What a server checks; for the app's self-test.
#[uniffi::export]
pub fn device_key_verify(public_key_x963: Vec<u8>, payload: Vec<u8>, signature: Vec<u8>) -> bool {
    PublicKey::from_sec1(&public_key_x963)
        .and_then(|k| k.verify_raw(&payload, &signature))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tallycam_core::device_key::SoftwareKey;

    #[test]
    fn identity_payload_and_message_agree_with_the_server_side() {
        let key = SoftwareKey::from_secret(&[5; 32]).unwrap();
        let public = key.public_key();
        let x963 = public.to_sec1();
        let id = device_key_identity(x963.clone()).unwrap();
        assert_eq!(id.device_id, public.device_id());
        assert_eq!(id.public_key, public.to_base64());

        let payload =
            device_key_auth_payload("srv".into(), "nonce".into(), id.device_id.clone()).unwrap();
        let raw = key.sign_raw(&payload);
        assert!(device_key_verify(
            x963.clone(),
            payload.clone(),
            raw.to_vec()
        ));
        assert!(!device_key_verify(x963, b"other".to_vec(), raw.to_vec()));
        let text = device_key_auth_message(raw.to_vec()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "auth");
        assert_eq!(
            public.verify_auth(
                "srv",
                "nonce",
                &id.device_id,
                v["signature"].as_str().unwrap()
            ),
            Ok(())
        );
        assert!(matches!(
            device_key_auth_payload("a\nb".into(), "n".into(), id.device_id),
            Err(DeviceKeyError::Challenge)
        ));
        assert!(matches!(
            device_key_identity(vec![4, 1]),
            Err(DeviceKeyError::PublicKey)
        ));
    }
}
