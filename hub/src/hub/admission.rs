//! Admission: invites, device keys (challenge and proof), and forgetting a
//! device.

use super::*;

/// Why a device connection was not admitted (see [`Hub::admit`]). The
/// `Display` text is shown on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitError {
    /// Invites are required, the device is unknown and no token came along.
    NoInvite,
    /// The token is unknown, already used or expired.
    BadInvite,
    /// The hello has no device key (an app before device keys), and this
    /// server requires one: it requires invites, or the id is bound to a key.
    KeyRequired,
    /// The public key is unusable, or the device id is not derived from it.
    BadKey(String),
    /// The key did not sign the challenge (wrong key, replayed or missing
    /// signature).
    BadSignature,
    /// A new device, and the hub already lists [`MAX_DEVICES`].
    TooManyDevices,
    /// The device signed in for another host than the one this connection
    /// reached: a challenge passed on by another server, or a URL that
    /// differs from what the hub knows itself by.
    WrongHost,
}

impl fmt::Display for AdmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdmitError::NoInvite => f.write_str("this camera is not invited; scan a new code"),
            AdmitError::BadInvite => f.write_str("the invite is used or expired; scan a new code"),
            AdmitError::KeyRequired => {
                f.write_str("this server needs a newer TallyCam app (device keys); update the app")
            }
            AdmitError::BadKey(why) => write!(f, "the device key is not usable: {why}"),
            AdmitError::BadSignature => {
                f.write_str("the device key did not verify; forget the server and scan a new code")
            }
            AdmitError::TooManyDevices => {
                f.write_str("this server lists too many cameras; forget some on the server")
            }
            AdmitError::WrongHost => f.write_str(
                "this camera signed in to another server address; check the server address",
            ),
        }
    }
}

/// A `challenge` sent to a device that said hello with a public key. The
/// session keeps it until the device's `auth` and hands it to [`Hub::admit`].
/// The nonce is fresh for every challenge, so a signature works only once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub nonce: String,
    /// The hub's id ([`Hub::server_id`]).
    pub server: String,
}

impl Challenge {
    pub fn message(&self) -> ServerToDevice {
        ServerToDevice::Challenge {
            nonce: self.nonce.clone(),
            server: self.server.clone(),
        }
    }
}

/// What a device presented to prove its key: the challenge it was sent,
/// its `auth` signature and the host it signed for, and the host this
/// connection reached (`Host`, or a proxy's `X-Forwarded-Host`).
#[derive(Debug, Clone, Copy)]
pub struct Proof<'a> {
    pub challenge: &'a Challenge,
    pub signature: &'a str,
    /// `auth.host`: the host the device signed for.
    pub host: &'a str,
    /// The host the connection came in by; None: unknown, refused.
    pub reached: Option<&'a str>,
}

impl std::error::Error for AdmitError {}

impl Hub {
    // -----------------------------------------------------------------------
    // Invites
    // -----------------------------------------------------------------------

    /// Require an invite token and a device key from devices the hub does
    /// not know yet. Known devices that prove their key are always admitted.
    /// Off by default, as the standalone hub on a LAN has always been open.
    pub fn set_invite_required(&self, required: bool) {
        self.lock().invite_required = required;
    }

    pub fn invite_required(&self) -> bool {
        self.lock().invite_required
    }

    /// A new one-time invite token, valid for `ttl`. It goes in the control
    /// URL handed to the phone (see [`crate::provision::with_invite_token`]).
    pub fn create_invite(&self, ttl: Duration) -> String {
        let token = random_token();
        let now = Instant::now();
        let mut inner = self.lock();
        inner.invites.retain(|_, expires| *expires > now);
        inner.invites.insert(token.clone(), now + ttl);
        token
    }

    /// This hub's id, sent in every `challenge`.
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// A fresh challenge for a device that said hello with a public key.
    pub fn challenge(&self) -> Challenge {
        let mut nonce = random_token();
        nonce.push_str(&random_token());
        Challenge {
            nonce,
            server: self.server_id.clone(),
        }
    }

    /// May the device that said `hello`, with `proof` of its key (when the
    /// hello carries one) and the invite `token` of its connection, join?
    /// Call it before [`Self::device_join`], which takes the returned
    /// [`Admission`]; it consumes the token when the token is what lets the
    /// device in.
    ///
    /// - With a key: the id must be derived from the key and the key must
    ///   have signed the challenge. A device known with this key is admitted.
    ///   Unknown devices, and devices known without a key, need an invite
    ///   when invites are required; the key is bound to the id on admission.
    /// - Without a key (an app before device keys): admitted only while
    ///   invites are off, and never for an id bound to a key.
    pub fn admit(
        &self,
        hello: &Hello,
        proof: Option<Proof<'_>>,
        token: Option<&str>,
    ) -> Result<Admission, AdmitError> {
        let id = hello.device.id.as_str();
        let Some(key_text) = hello.public_key.as_deref() else {
            let inner = self.lock();
            inner.check_room(id)?;
            let bound = inner
                .devices
                .get(id)
                .is_some_and(|e| e.public_key.is_some());
            if inner.invite_required || bound {
                return Err(AdmitError::KeyRequired);
            }
            return Ok(Admission {
                key: None,
                forgets: inner.forgets(id),
            });
        };
        let key = hello_key(key_text, id)?;
        let proof = proof.ok_or(AdmitError::BadSignature)?;
        if proof.challenge.server != self.server_id {
            return Err(AdmitError::BadSignature);
        }
        // The device signed the host it dialled; a server that relays the
        // challenge got this connection under its own host instead.
        let reached = proof.reached.map(crate::device_key::normalize_host);
        if reached.as_deref() != Some(crate::device_key::normalize_host(proof.host).as_str()) {
            warn!(
                "Device {id}: signed for host {:?}, connection reached {:?}",
                proof.host, proof.reached
            );
            return Err(AdmitError::WrongHost);
        }
        key.verify_auth(
            &proof.challenge.server,
            &proof.challenge.nonce,
            id,
            proof.host,
            proof.signature,
        )
        .map_err(|e: KeyError| {
            debug!("Device {id}: {e}");
            AdmitError::BadSignature
        })?;

        let now = Instant::now();
        let mut inner = self.lock();
        let admission = Admission {
            key: Some(key_text.to_string()),
            forgets: inner.forgets(id),
        };
        match inner
            .devices
            .get(id)
            .and_then(|e| e.public_key.as_ref().map(|_| e))
        {
            Some(e) if e.key.as_ref().map(|(k, _)| k) == Some(&key) => {
                return Ok(admission);
            }
            // Ids are derived from keys, so this takes a store edited by hand.
            Some(_) => return Err(AdmitError::BadSignature),
            None => {}
        }
        inner.check_room(id)?;
        if !inner.invite_required {
            return Ok(admission);
        }
        inner.invites.retain(|_, expires| *expires > now);
        let token = token
            .filter(|t| !t.is_empty())
            .ok_or(AdmitError::NoInvite)?;
        match inner.invites.remove(token) {
            Some(_) => {
                info!("Device {id} admitted with an invite, key bound");
                Ok(admission)
            }
            None => Err(AdmitError::BadInvite),
        }
    }

    /// Forget a device: it is removed from the list and the store, its
    /// connection is closed, and its key is no longer accepted. To come back
    /// it needs a new invite (when invites are required). Returns false for
    /// an unknown device.
    pub fn forget(&self, device: &str) -> bool {
        {
            let mut inner = self.lock();
            let Some(entry) = inner.devices.remove(device) else {
                return false;
            };
            inner.order.retain(|id| id != device);
            inner.drop_pending_for(device);
            // An admission that passed before this cannot join any more.
            *inner.forgets.entry(device.to_string()).or_default() += 1;
            // Dropping the entry drops the connection's sender; its writer
            // then closes the socket.
            drop(entry);
            info!("Device {device} forgotten");
            self.emit(ControllerEvent::DeviceRemoved {
                device: device.to_string(),
            });
            // Under the lock, so a concurrent join cannot save it back.
            self.store.remove(device);
        }
        true
    }
}

/// What [`Hub::admit`] let in, for [`Hub::device_join`]: the key that was
/// verified (bound on join; never the key a later `hello` carries) and the
/// device's forget count at admission, so a device forgotten in between
/// cannot join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub(super) key: Option<String>,
    pub(super) forgets: u64,
}

impl Inner {
    /// Is there room for `id` in the device list?
    pub(super) fn check_room(&self, id: &str) -> Result<(), AdmitError> {
        if self.devices.len() >= MAX_DEVICES && !self.devices.contains_key(id) {
            warn!("Device {id} refused: {MAX_DEVICES} devices listed");
            return Err(AdmitError::TooManyDevices);
        }
        Ok(())
    }
}

/// Checks the public key of a `hello` before it is challenged: a usable key,
/// and the device id derived from it. `Ok(None)` for a hello without a key.
pub fn check_hello_key(hello: &Hello) -> Result<Option<PublicKey>, AdmitError> {
    hello
        .public_key
        .as_deref()
        .map(|k| hello_key(k, &hello.device.id))
        .transpose()
}

fn hello_key(key: &str, id: &str) -> Result<PublicKey, AdmitError> {
    let key = PublicKey::from_base64(key).map_err(|e| AdmitError::BadKey(e.to_string()))?;
    if key.device_id() != id {
        return Err(AdmitError::BadKey(
            "the device id is not derived from the key".into(),
        ));
    }
    Ok(key)
}

/// 128 random bits as 32 lowercase hex characters.
pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("no system random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
