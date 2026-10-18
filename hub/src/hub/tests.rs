use super::*;
use crate::device_key::SoftwareKey;
use crate::protocol::ShutterPolicy;

/// Records what the hub saves.
#[derive(Default)]
struct RecordingStore {
    initial: Vec<StoredDevice>,
    saved: Mutex<Vec<StoredDevice>>,
    removed: Mutex<Vec<String>>,
    production: Mutex<Option<Production>>,
    order: Mutex<Option<Vec<String>>>,
}

impl DeviceStore for RecordingStore {
    fn load(&self) -> Vec<StoredDevice> {
        self.initial.clone()
    }
    fn save(&self, device: &StoredDevice) {
        self.saved.lock().unwrap().push(device.clone());
    }
    fn remove(&self, id: &str) {
        self.removed.lock().unwrap().push(id.to_string());
    }
    fn load_production(&self) -> Option<Production> {
        self.production.lock().unwrap().clone()
    }
    fn save_production(&self, production: &Production) {
        *self.production.lock().unwrap() = Some(production.clone());
    }
    fn save_order(&self, ids: &[String]) {
        *self.order.lock().unwrap() = Some(ids.to_vec());
    }
}

fn info(id: &str, name: &str) -> DeviceInfo {
    serde_json::from_value(serde_json::json!({ "id": id, "name": name })).unwrap()
}

fn hello(id: &str, name: &str) -> Hello {
    Hello {
        protocol: PROTOCOL_VERSION,
        device: info(id, name),
        public_key: None,
        controls: Vec::new(),
        commands: Vec::new(),
    }
}

/// A hello from a phone with `key`, its id derived from the key.
fn keyed_hello(key: &SoftwareKey, name: &str) -> Hello {
    let public = key.public_key();
    Hello {
        public_key: Some(public.to_base64()),
        ..hello(&public.device_id(), name)
    }
}

/// Challenge `hello`'s phone and admit it with `key`'s signature.
fn admit_signed(
    hub: &Hub,
    hello: &Hello,
    key: &SoftwareKey,
    token: Option<&str>,
) -> Result<(), AdmitError> {
    admit_ticket(hub, hello, key, token).map(|_| ())
}

fn admit_ticket(
    hub: &Hub,
    hello: &Hello,
    key: &SoftwareKey,
    token: Option<&str>,
) -> Result<Admission, AdmitError> {
    let challenge = hub.challenge();
    let payload =
        crate::device_key::auth_payload(&challenge.server, &challenge.nonce, &hello.device.id)
            .unwrap();
    let signature = key.sign(&payload);
    hub.admit(
        hello,
        Some(Proof {
            challenge: &challenge,
            signature: &signature,
        }),
        token,
    )
}

/// A device without a key joining an open hub, as a phone before device keys.
fn join(hub: &Hub, conn: u64, hello: Hello, tx: DeviceTx) {
    let admission = hub.admit(&hello, None, None).unwrap();
    hub.device_join(conn, hello, admission, tx, None).unwrap();
}

fn stored(id: &str, name: &str) -> StoredDevice {
    StoredDevice {
        info: info(id, name),
        public_key: None,
        tags: Vec::new(),
        last_seen_s: None,
    }
}

#[test]
fn stored_devices_are_listed_offline_and_hello_saves_changes() {
    let store = Arc::new(RecordingStore {
        initial: vec![stored("cam1", "Cam 1")],
        ..Default::default()
    });
    let hub = Hub::with_store(store.clone());
    let devices = hub.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].info.name, "Cam 1");
    assert!(!devices[0].online);
    assert_eq!(devices[0].tally, TallyState::Off);

    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    let conn1 = hub.new_connection_id();
    join(&hub, conn1, hello("cam1", "Cam 1"), tx.clone());
    // Saved for when it was seen, with unchanged info.
    {
        let saved = store.saved.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].info.name, "Cam 1");
        assert!(saved[0].last_seen_s.is_some());
    }
    // A resent hello with the same info is not saved.
    hub.device_hello(conn1, hello("cam1", "Cam 1"));
    assert_eq!(store.saved.lock().unwrap().len(), 1);
    store.saved.lock().unwrap().clear();

    let (tx2, _rx2) = mpsc::channel(DEVICE_QUEUE);
    join(&hub, hub.new_connection_id(), hello("cam2", "Cam 2"), tx2);
    let (tx3, _rx3) = mpsc::channel(DEVICE_QUEUE);
    join(
        &hub,
        hub.new_connection_id(),
        hello("cam1", "Stage left"),
        tx3,
    );
    let saved: Vec<_> = store
        .saved
        .lock()
        .unwrap()
        .iter()
        .map(|d| d.info.name.clone())
        .collect();
    assert_eq!(saved, ["Cam 2", "Stage left"]);
    let order: Vec<_> = hub.devices().into_iter().map(|d| d.id).collect();
    assert_eq!(order, ["cam1", "cam2"]);
}

#[test]
fn set_tally_reaches_phone_and_subscribers_once_per_change() {
    let hub = Hub::new();
    assert!(hub.set_tally("nope", TallyState::Program).is_err());

    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    join(&hub, hub.new_connection_id(), hello("cam1", "Cam 1"), tx);
    // hello brings the phone up to date: tally, then preview.
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Tally {
            state: TallyState::Off
        })
    ));
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Preview { .. })
    ));
    // A protocol 2 phone also gets the production and the stats preference.
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Production(_))
    ));
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Stats { enabled: false, .. })
    ));

    let mut events = hub.subscribe();
    hub.set_tally("cam1", TallyState::Program).unwrap();
    hub.set_tally("cam1", TallyState::Program).unwrap();

    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Tally {
            state: TallyState::Program
        })
    ));
    assert!(phone.try_recv().is_err(), "a repeated tally is not resent");

    let frame = events.try_recv().unwrap();
    assert_eq!(
        *frame.event,
        ControllerEvent::DeviceTally {
            device: "cam1".into(),
            state: TallyState::Program
        }
    );
    assert!(frame.json.contains(r#""type":"device_tally""#));
    assert!(events.try_recv().is_err(), "a repeated tally emits nothing");
}

#[test]
fn tally_set_while_offline_is_sent_on_connect() {
    let store = Arc::new(RecordingStore {
        initial: vec![stored("cam1", "Cam 1")],
        ..Default::default()
    });
    let hub = Hub::with_store(store);
    hub.set_tally("cam1", TallyState::Preview).unwrap();

    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    join(&hub, hub.new_connection_id(), hello("cam1", "Cam 1"), tx);
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Tally {
            state: TallyState::Preview
        })
    ));
}

/// A connected phone that answers with what `answer` returns.
fn connect_phone(hub: &Hub, id: &str) -> mpsc::Receiver<ServerToDevice> {
    let (tx, mut rx) = mpsc::channel(DEVICE_QUEUE);
    let conn = hub.new_connection_id();
    join(hub, conn, hello(id, id), tx);
    // Drain what hello sends (tally, preview).
    while rx.try_recv().is_ok() {}
    // Remember the connection for replies.
    CONNS.with(|c| c.borrow_mut().insert(id.to_string(), conn));
    rx
}

thread_local! {
    static CONNS: std::cell::RefCell<HashMap<String, u64>> = Default::default();
}

fn reply(hub: &Hub, device: &str, id: Value, ok: bool, error: Option<&str>) {
    let conn = CONNS.with(|c| c.borrow()[device]);
    hub.device_message(
        conn,
        device,
        DeviceMessage::Result(CommandResult {
            id,
            ok,
            error: error.map(str::to_string),
            data: None,
        }),
    );
}

#[tokio::test]
async fn set_waits_for_the_matching_result() {
    let hub = Arc::new(Hub::new());
    let mut phone = connect_phone(&hub, "cam1");
    let mut events = hub.subscribe();

    let mut values = Values::new();
    values.insert("live".into(), Value::Bool(true));
    let call = {
        let hub = hub.clone();
        tokio::spawn(async move { hub.set("cam1", values, Duration::from_secs(2)).await })
    };
    let sent = phone.recv().await.unwrap();
    let ServerToDevice::Set {
        id: Some(id),
        values,
    } = sent
    else {
        panic!("expected a set, got {sent:?}")
    };
    assert_eq!(values["live"], Value::Bool(true));
    assert!(id.as_str().unwrap().starts_with(CALL_ID_PREFIX));

    // A controller's result with another id does not complete the call.
    reply(&hub, "cam1", Value::from(7), true, None);
    reply(&hub, "cam1", id.clone(), true, None);
    let result = call.await.unwrap().unwrap();
    assert_eq!(result.id, id);
    assert!(hub.lock().pending.is_empty());
    // Controllers saw both results.
    assert!(matches!(
        &*events.recv().await.unwrap().event,
        ControllerEvent::DeviceResult { .. }
    ));
    assert!(matches!(
        &*events.recv().await.unwrap().event,
        ControllerEvent::DeviceResult { .. }
    ));
}

#[tokio::test]
async fn command_failure_timeout_and_offline() {
    let hub = Arc::new(Hub::new());
    assert_eq!(
        hub.command("nope", "x", None, Duration::from_millis(10))
            .await,
        Err(CallError::UnknownDevice("nope".into()))
    );
    let mut phone = connect_phone(&hub, "cam1");

    let call = {
        let hub = hub.clone();
        tokio::spawn(async move {
            hub.command("cam1", "focus", None, Duration::from_secs(2))
                .await
        })
    };
    let ServerToDevice::Command {
        id: Some(id), name, ..
    } = phone.recv().await.unwrap()
    else {
        panic!("expected a command")
    };
    assert_eq!(name, "focus");
    reply(&hub, "cam1", id, false, Some("no lens"));
    assert_eq!(
        call.await.unwrap(),
        Err(CallError::Failed {
            device: "cam1".into(),
            error: Some("no lens".into())
        })
    );

    assert_eq!(
        hub.set("cam1", Values::new(), Duration::from_millis(20))
            .await,
        Err(CallError::Timeout("cam1".into()))
    );
    assert!(
        hub.lock().pending.is_empty(),
        "a timed out call is forgotten"
    );
    assert!(matches!(phone.try_recv(), Ok(ServerToDevice::Set { .. })));

    // A disconnect fails the waiting call at once.
    let call = {
        let hub = hub.clone();
        tokio::spawn(async move { hub.set("cam1", Values::new(), Duration::from_secs(5)).await })
    };
    let _ = phone.recv().await.unwrap();
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    hub.device_disconnected(conn, "cam1");
    assert_eq!(
        call.await.unwrap(),
        Err(CallError::Disconnected("cam1".into()))
    );
    assert_eq!(
        hub.set("cam1", Values::new(), Duration::from_millis(10))
            .await,
        Err(CallError::Offline("cam1".into()))
    );
    assert_eq!(
        hub.message("cam1", "hi"),
        Err(CallError::Offline("cam1".into()))
    );
}

#[test]
fn message_reaches_the_phone() {
    let hub = Hub::new();
    let mut phone = connect_phone(&hub, "cam1");
    hub.message("cam1", "Stand by").unwrap();
    assert_eq!(
        phone.try_recv().unwrap(),
        ServerToDevice::Message {
            text: "Stand by".into()
        }
    );
}

#[test]
fn legacy_devices_are_admitted_only_while_invites_are_off() {
    let hub = Hub::new();
    // Off by default: an app without a key is admitted.
    assert_eq!(
        hub.admit(&hello("cam1", "Cam 1"), None, None).map(|_| ()),
        Ok(())
    );

    hub.set_invite_required(true);
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    assert_eq!(
        hub.admit(&hello("cam1", "Cam 1"), None, Some(&token)),
        Err(AdmitError::KeyRequired),
        "no key, no entry, even with an invite"
    );
    assert!(AdmitError::KeyRequired
        .to_string()
        .contains("update the app"));

    // An id bound to a key never takes a hello without one.
    let hub = Hub::new();
    let key = crate::device_key::generate();
    let keyed = keyed_hello(&key, "Cam K");
    let ticket = admit_ticket(&hub, &keyed, &key, None).unwrap();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(hub.new_connection_id(), keyed.clone(), ticket, tx, None)
        .unwrap();
    let impostor = hello(&keyed.device.id, "Impostor");
    assert_eq!(
        hub.admit(&impostor, None, None),
        Err(AdmitError::KeyRequired)
    );
}

#[test]
fn an_invite_binds_the_key_and_the_key_is_enough_later() {
    let store = Arc::new(RecordingStore::default());
    let hub = Hub::with_store(store.clone());
    hub.set_invite_required(true);
    let key = crate::device_key::generate();
    let h = keyed_hello(&key, "Cam 1");

    // Unknown device without a token: refused, even with a good signature.
    assert_eq!(
        admit_signed(&hub, &h, &key, None),
        Err(AdmitError::NoInvite)
    );
    assert_eq!(
        admit_signed(&hub, &h, &key, Some("bogus")),
        Err(AdmitError::BadInvite)
    );

    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    assert_eq!(token.len(), 32);
    let ticket = admit_ticket(&hub, &h, &key, Some(&token)).unwrap();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(hub.new_connection_id(), h.clone(), ticket, tx, None)
        .unwrap();
    let saved = store.saved.lock().unwrap().clone();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].public_key, h.public_key, "the key is stored");
    let snap = &hub.devices()[0];
    assert_eq!(snap.key_fingerprint, Some(key.public_key().fingerprint()));

    // Known now: the key alone admits it, the spent token is not needed.
    assert_eq!(admit_signed(&hub, &h, &key, None), Ok(()));
    assert_eq!(admit_signed(&hub, &h, &key, Some(&token)), Ok(()));
    // The spent token does not admit another phone.
    let other = crate::device_key::generate();
    assert_eq!(
        admit_signed(&hub, &keyed_hello(&other, "Other"), &other, Some(&token)),
        Err(AdmitError::BadInvite)
    );
    // Nor an expired one.
    let expired = hub.create_invite(Duration::ZERO);
    assert_eq!(
        admit_signed(&hub, &keyed_hello(&other, "Other"), &other, Some(&expired)),
        Err(AdmitError::BadInvite)
    );

    // After a restart the stored key still admits it without a token.
    let hub = Hub::with_store(Arc::new(RecordingStore {
        initial: saved,
        ..Default::default()
    }));
    hub.set_invite_required(true);
    assert_eq!(admit_signed(&hub, &h, &key, None), Ok(()));
}

#[test]
fn a_known_device_needs_its_key() {
    let hub = Hub::new();
    hub.set_invite_required(true);
    let key = crate::device_key::generate();
    let h = keyed_hello(&key, "Cam 1");
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    let ticket = admit_ticket(&hub, &h, &key, Some(&token)).unwrap();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(hub.new_connection_id(), h.clone(), ticket, tx, None)
        .unwrap();

    // Its id with another key: the id is not derived from that key.
    let thief = crate::device_key::generate();
    let stolen = Hello {
        public_key: Some(thief.public_key().to_base64()),
        ..h.clone()
    };
    assert!(matches!(
        admit_signed(&hub, &stolen, &thief, None),
        Err(AdmitError::BadKey(_))
    ));
    // Its key, signed by another key.
    assert_eq!(
        admit_signed(&hub, &h, &thief, None),
        Err(AdmitError::BadSignature)
    );
    // Its key without any signature.
    assert_eq!(hub.admit(&h, None, None), Err(AdmitError::BadSignature));
    // Garbage key.
    let garbage = Hello {
        public_key: Some("bm8=".into()),
        ..h.clone()
    };
    assert!(matches!(
        hub.admit(&garbage, None, None),
        Err(AdmitError::BadKey(_))
    ));
}

#[test]
fn a_signature_works_for_one_challenge_only() {
    let hub = Hub::new();
    hub.set_invite_required(true);
    let key = crate::device_key::generate();
    let h = keyed_hello(&key, "Cam 1");
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    let ticket = admit_ticket(&hub, &h, &key, Some(&token)).unwrap();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(hub.new_connection_id(), h.clone(), ticket, tx, None)
        .unwrap();

    // A signature captured from one session...
    let first = hub.challenge();
    let signature = key.sign_auth(&first.server, &first.nonce);
    let ok = Proof {
        challenge: &first,
        signature: &signature,
    };
    assert_eq!(hub.admit(&h, Some(ok), None).map(|_| ()), Ok(()));
    // ...does not answer the next challenge.
    let second = hub.challenge();
    assert_ne!(first.nonce, second.nonce);
    let replay = Proof {
        challenge: &second,
        signature: &signature,
    };
    assert_eq!(
        hub.admit(&h, Some(replay), None),
        Err(AdmitError::BadSignature)
    );
    // Nor a challenge from another server.
    let other_hub = Hub::new();
    let foreign = other_hub.challenge();
    let signature = key.sign_auth(&foreign.server, &foreign.nonce);
    assert_eq!(
        hub.admit(
            &h,
            Some(Proof {
                challenge: &foreign,
                signature: &signature
            }),
            None
        ),
        Err(AdmitError::BadSignature)
    );
}

#[test]
fn a_device_stored_without_a_key_needs_a_new_invite() {
    let key = crate::device_key::generate();
    let id = key.public_key().device_id();
    let hub = Hub::with_store(Arc::new(RecordingStore {
        initial: vec![stored(&id, "Old")],
        ..Default::default()
    }));
    hub.set_invite_required(true);
    let h = keyed_hello(&key, "Old");
    assert_eq!(
        admit_signed(&hub, &h, &key, None),
        Err(AdmitError::NoInvite)
    );
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    assert_eq!(admit_signed(&hub, &h, &key, Some(&token)), Ok(()));
}

#[test]
fn forget_closes_the_connection_and_drops_the_key() {
    let store = Arc::new(RecordingStore::default());
    let hub = Hub::with_store(store.clone());
    hub.set_invite_required(true);
    let key = crate::device_key::generate();
    let h = keyed_hello(&key, "Cam 1");
    let id = h.device.id.clone();
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    let ticket = admit_ticket(&hub, &h, &key, Some(&token)).unwrap();
    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(hub.new_connection_id(), h.clone(), ticket, tx, None)
        .unwrap();
    while phone.try_recv().is_ok() {}
    let mut events = hub.subscribe();

    assert!(!hub.forget("nope"));
    assert!(hub.forget(&id));
    assert!(hub.devices().is_empty());
    assert_eq!(
        store.removed.lock().unwrap().as_slice(),
        std::slice::from_ref(&id)
    );
    assert_eq!(
        *events.try_recv().unwrap().event,
        ControllerEvent::DeviceRemoved { device: id.clone() }
    );
    // The hub dropped its sender: the session's writer ends.
    assert!(matches!(
        phone.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    // Its key alone no longer admits it.
    assert_eq!(
        admit_signed(&hub, &h, &key, None),
        Err(AdmitError::NoInvite)
    );
}

#[test]
fn a_resent_hello_never_changes_the_bound_key() {
    // Review finding: a hello resent on an admitted connection used to bind
    // whatever key it carried, without a challenge.
    let hub = Hub::new();
    let key = crate::device_key::generate();
    let h = keyed_hello(&key, "Cam 1");
    let id = h.device.id.clone();
    let ticket = admit_ticket(&hub, &h, &key, None).unwrap();
    let conn = hub.new_connection_id();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(conn, h.clone(), ticket, tx, None).unwrap();
    let bound = hub.lock().devices[&id].public_key.clone();
    assert_eq!(bound, h.public_key);

    let other = crate::device_key::generate();
    let mut resent = h.clone();
    resent.public_key = keyed_hello(&other, "x").public_key;
    resent.device.name = "Renamed".into();
    hub.device_hello(conn, resent);
    let inner = hub.lock();
    assert_eq!(inner.devices[&id].public_key, bound, "key unchanged");
    assert_eq!(
        inner.devices[&id].info.name, "Renamed",
        "info still updates"
    );
}

#[test]
fn a_device_forgotten_after_admission_cannot_join() {
    // Review finding: admit and join were two lock sections, so a device
    // forgotten in between came back, in the hub and in the store.
    let store = Arc::new(RecordingStore::default());
    let hub = Hub::with_store(store.clone());
    hub.set_invite_required(true);
    let key = crate::device_key::generate();
    let h = keyed_hello(&key, "Cam 1");
    let token = hub.create_invite(DEFAULT_INVITE_TTL);
    let ticket = admit_ticket(&hub, &h, &key, Some(&token)).unwrap();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    hub.device_join(hub.new_connection_id(), h.clone(), ticket, tx, None)
        .unwrap();

    // It reconnects: admitted by its key, then forgotten before it joins.
    let ticket = admit_ticket(&hub, &h, &key, None).unwrap();
    assert!(hub.forget(&h.device.id));
    let saved_before = store.saved.lock().unwrap().len();
    let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
    assert_eq!(
        hub.device_join(hub.new_connection_id(), h.clone(), ticket, tx, None),
        Err(AdmitError::NoInvite)
    );
    assert!(hub.devices().is_empty());
    assert_eq!(
        store.saved.lock().unwrap().len(),
        saved_before,
        "not saved back"
    );
}

// ---------------------------------------------------------------------------
// Shading: production, image statistics
// ---------------------------------------------------------------------------

fn hello_v1(id: &str, name: &str) -> Hello {
    Hello {
        protocol: 1,
        ..hello(id, name)
    }
}

/// What a controller frame does, through the same parsing as the server.
fn controller(hub: &Hub, json: Value) -> Result<(), String> {
    match crate::protocol::ControllerInput::parse(&json.to_string())? {
        Some(crate::protocol::ControllerInput::Device(req)) => hub.controller_request(req),
        Some(crate::protocol::ControllerInput::Production(p)) => hub.set_production(p),
        Some(crate::protocol::ControllerInput::Tags { device, tags }) => {
            hub.set_tags(&device, tags).map(|_| ())
        }
        Some(crate::protocol::ControllerInput::Arrange(order)) => {
            hub.arrange(order);
            Ok(())
        }
        Some(crate::protocol::ControllerInput::Watch(_)) => {
            panic!("watch goes through a controller's guard")
        }
        None => Ok(()),
    }
}

fn report_stream(hub: &Hub, device: &str, state: &str) {
    let conn = CONNS.with(|c| c.borrow()[device]);
    let status: Status =
        serde_json::from_value(serde_json::json!({ "stream": { "state": state } })).unwrap();
    hub.device_message(conn, device, DeviceMessage::Status(status));
}

#[test]
fn exposure_colour_and_focus_are_plain_controls() {
    let hub = Hub::new();
    let mut phone = connect_phone(&hub, "cam1");
    let mut events = hub.subscribe();

    // Any controller sets any control; the last write wins on the phone.
    controller(
        &hub,
        serde_json::json!({ "type": "set", "device": "cam1",
                            "values": { "iso": 400, "wb_temperature": 5600, "shutter": 100 } }),
    )
    .unwrap();
    let Ok(ServerToDevice::Set { values, .. }) = phone.try_recv() else {
        panic!("expected the set")
    };
    assert_eq!(values["iso"], 400);
    controller(
        &hub,
        serde_json::json!({ "type": "command", "device": "cam1", "name": "focus_at",
                            "args": { "x": 0.5, "y": 0.5 } }),
    )
    .unwrap();
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Command { .. })
    ));

    // An older controller's `holder` is ignored, its `own` and `release`
    // are unknown types.
    controller(
        &hub,
        serde_json::json!({ "type": "set", "device": "cam1", "values": { "iso": 200 },
                            "holder": { "id": "a", "label": "Shader A" } }),
    )
    .unwrap();
    assert!(matches!(phone.try_recv(), Ok(ServerToDevice::Set { .. })));
    for ty in ["own", "release"] {
        controller(
            &hub,
            serde_json::json!({ "type": ty, "device": "cam1", "groups": ["exposure"],
                                "holder": { "id": "a", "label": "A" } }),
        )
        .unwrap();
    }
    assert!(
        phone.try_recv().is_err(),
        "own and release are not forwarded"
    );

    // An older phone's `ownership` is ignored.
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    let ownership: DeviceMessage =
        serde_json::from_str(r#"{"type":"ownership","groups":{"exposure":{"owner":"local"}}}"#)
            .unwrap();
    hub.device_message(conn, "cam1", ownership);
    assert!(events.try_recv().is_err(), "nothing forwarded");
    let snapshot = serde_json::to_value(hub.devices()).unwrap();
    assert!(snapshot[0].get("ownership").is_none());
}

#[test]
fn protocol_1_devices_get_no_shading() {
    let hub = Hub::new();
    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    let conn = hub.new_connection_id();
    join(&hub, conn, hello_v1("old", "Old app"), tx);
    CONNS.with(|c| c.borrow_mut().insert("old".into(), conn));
    // Admission sends tally and preview only.
    assert!(matches!(phone.try_recv(), Ok(ServerToDevice::Tally { .. })));
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Preview { .. })
    ));
    assert!(phone.try_recv().is_err(), "no production or stats");

    controller(
        &hub,
        serde_json::json!({ "type": "set", "device": "old", "values": { "iso": 400, "wb_tint": 3 } }),
    )
    .unwrap();
    assert!(matches!(phone.try_recv(), Ok(ServerToDevice::Set { .. })));
    hub.set_production(Production {
        mains_hz: 60,
        ..default_production()
    })
    .unwrap();
    assert!(
        phone.try_recv().is_err(),
        "production goes to protocol 2 only"
    );
    assert_eq!(hub.devices()[0].protocol, 1);
}

#[test]
fn production_is_sent_on_admission_and_accepted_while_live() {
    let hub = Hub::new();
    let mut events = hub.subscribe();
    hub.set_production(Production {
        mains_hz: 60,
        shutter: ShutterPolicy::Fixed(120),
        extra: Map::new(),
    })
    .unwrap();
    assert!(matches!(
        &*events.try_recv().unwrap().event,
        ControllerEvent::Production(p) if p.mains_hz == 60
    ));

    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    let conn = hub.new_connection_id();
    join(&hub, conn, hello("cam1", "Cam 1"), tx);
    CONNS.with(|c| c.borrow_mut().insert("cam1".into(), conn));
    let sent: Vec<_> = std::iter::from_fn(|| phone.try_recv().ok()).collect();
    assert!(sent.iter().any(|m| matches!(
        m,
        ServerToDevice::Production(p) if p.mains_hz == 60 && p.shutter == ShutterPolicy::Fixed(120)
    )));

    // Live: the phone gets it and applies it when its stream stops.
    report_stream(&hub, "cam1", "live");
    controller(
        &hub,
        serde_json::json!({ "type": "production", "mains_hz": 50, "shutter": "mains" }),
    )
    .unwrap();
    assert_eq!(
        phone.try_recv().unwrap(),
        ServerToDevice::Production(default_production())
    );
    assert_eq!(hub.production(), default_production());
    // Unchanged settings send nothing.
    hub.set_production(hub.production()).unwrap();
    assert!(phone.try_recv().is_err());

    // Values are still checked.
    assert!(hub
        .set_production(Production {
            mains_hz: 55,
            ..default_production()
        })
        .is_err());
    assert!(hub
        .set_production(Production {
            shutter: ShutterPolicy::Fixed(0),
            ..default_production()
        })
        .is_err());
    assert_eq!(hub.production(), default_production());
}

#[test]
fn production_is_kept_in_the_store() {
    let store = Arc::new(RecordingStore::default());
    let hub = Hub::with_store(store.clone());
    assert_eq!(hub.production(), default_production());
    let sixty = Production {
        mains_hz: 60,
        shutter: ShutterPolicy::Free,
        extra: Map::new(),
    };
    hub.set_production(sixty.clone()).unwrap();
    assert_eq!(*store.production.lock().unwrap(), Some(sixty.clone()));

    // A restart starts with it.
    let again = Hub::with_store(store.clone());
    assert_eq!(again.production(), sixty);

    // One the hub would refuse (an edited state file) is not used.
    *store.production.lock().unwrap() = Some(Production {
        mains_hz: 55,
        ..sixty
    });
    let edited = Hub::with_store(store);
    assert_eq!(edited.production(), default_production());
}

#[test]
fn going_offline_drops_status_and_image_stats() {
    let hub = Hub::new();
    let _phone = connect_phone(&hub, "cam1");
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    report_stream(&hub, "cam1", "live");
    let stats: ImageStats =
        serde_json::from_value(serde_json::json!({ "luma": { "mean": 0.4 } })).unwrap();
    hub.device_message(conn, "cam1", DeviceMessage::ImageStats(stats));
    assert!(hub.devices()[0].status.is_some());
    assert!(hub.devices()[0].image_stats.is_some());

    hub.device_disconnected(conn, "cam1");
    let snap = &hub.devices()[0];
    assert!(!snap.online);
    assert_eq!(snap.status, None, "a stale stream state would read as live");
    assert_eq!(snap.image_stats, None);

    // Back online: nothing stale until the phone reports again.
    let _phone = connect_phone(&hub, "cam1");
    assert_eq!(hub.devices()[0].status, None);
}

#[test]
fn image_stats_follow_the_controllers() {
    let hub = Arc::new(Hub::new());
    let mut v2 = connect_phone(&hub, "cam1");
    let (tx, mut v1) = mpsc::channel(DEVICE_QUEUE);
    join(&hub, hub.new_connection_id(), hello_v1("old", "Old"), tx);
    while v1.try_recv().is_ok() {}

    let (snapshot, _rx, guard) = hub.controller_joined();
    assert!(snapshot.contains(r#""production":{"mains_hz":50,"shutter":"mains"}"#));
    assert!(matches!(
        v2.try_recv(),
        Ok(ServerToDevice::Preview { enabled: true, .. })
    ));
    assert_eq!(
        v2.try_recv().unwrap(),
        ServerToDevice::Stats {
            enabled: true,
            fps: Some(DEFAULT_STATS_FPS)
        }
    );
    assert!(matches!(
        v1.try_recv(),
        Ok(ServerToDevice::Preview { enabled: true, .. })
    ));
    assert!(v1.try_recv().is_err(), "no stats for protocol 1");

    // A second controller changes nothing; a controller's rate is clamped.
    let (_, _rx2, guard2) = hub.controller_joined();
    assert!(v2.try_recv().is_err());
    controller(
        &hub,
        serde_json::json!({ "type": "stats", "device": "cam1", "enabled": true, "fps": 10 }),
    )
    .unwrap();
    assert_eq!(
        v2.try_recv().unwrap(),
        ServerToDevice::Stats {
            enabled: true,
            fps: Some(4.0)
        }
    );

    // The device's statistics are kept for the snapshot.
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    let stats: ImageStats =
        serde_json::from_value(serde_json::json!({ "luma": { "mean": 0.4, "clipped": 0.01 } }))
            .unwrap();
    hub.device_message(conn, "cam1", DeviceMessage::ImageStats(stats.clone()));
    assert_eq!(hub.devices()[0].image_stats, Some(stats.clone()));

    // Switched off by a controller: the numbers go stale, so they go.
    controller(
        &hub,
        serde_json::json!({ "type": "stats", "device": "cam1", "enabled": false }),
    )
    .unwrap();
    assert!(matches!(
        v2.try_recv(),
        Ok(ServerToDevice::Stats { enabled: false, .. })
    ));
    assert_eq!(hub.devices()[0].image_stats, None);
    controller(
        &hub,
        serde_json::json!({ "type": "stats", "device": "cam1", "enabled": true }),
    )
    .unwrap();
    assert!(matches!(
        v2.try_recv(),
        Ok(ServerToDevice::Stats { enabled: true, .. })
    ));
    hub.device_message(conn, "cam1", DeviceMessage::ImageStats(stats));

    drop(guard2);
    assert!(v2.try_recv().is_err());
    drop(guard);
    assert!(matches!(
        v2.try_recv(),
        Ok(ServerToDevice::Preview { enabled: false, .. })
    ));
    assert_eq!(
        v2.try_recv().unwrap(),
        ServerToDevice::Stats {
            enabled: false,
            fps: None
        }
    );
    assert!(matches!(
        v1.try_recv(),
        Ok(ServerToDevice::Preview { enabled: false, .. })
    ));
    assert!(v1.try_recv().is_err());
}

#[test]
fn controllers_never_see_the_srt_passphrase() {
    let hub = Hub::new();
    let mut phone = connect_phone(&hub, "cam1");
    let mut events = hub.subscribe();
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    let url = "srt://strom:9000?streamid=cam1&passphrase=0123456789abc&latency=100";
    let mut values = Values::new();
    values.insert("stream_url".into(), Value::String(url.into()));
    hub.device_message(conn, "cam1", DeviceMessage::State { values });

    let hidden = "srt://strom:9000?streamid=cam1&passphrase=***&latency=100";
    let frame = events.try_recv().unwrap();
    let ControllerEvent::DeviceState { values, .. } = &*frame.event else {
        panic!("expected the state event")
    };
    assert_eq!(values["stream_url"], hidden);
    assert_eq!(hub.devices()[0].state["stream_url"], hidden);

    // Sent back edited, the phone gets its own passphrase.
    controller(
        &hub,
        serde_json::json!({ "type": "set", "device": "cam1", "values": {
            "stream_url": "srt://strom:9000?streamid=cam1&passphrase=***&latency=200" } }),
    )
    .unwrap();
    let Ok(ServerToDevice::Set { values, .. }) = phone.try_recv() else {
        panic!("expected the set")
    };
    assert_eq!(
        values["stream_url"],
        "srt://strom:9000?streamid=cam1&passphrase=0123456789abc&latency=200"
    );
}

#[test]
fn the_device_list_and_each_state_are_bounded() {
    let hub = Hub::new();
    for i in 0..MAX_DEVICES {
        let (tx, _rx) = mpsc::channel(DEVICE_QUEUE);
        join(
            &hub,
            hub.new_connection_id(),
            hello(&format!("d{i}"), "x"),
            tx,
        );
    }
    let extra = hello("one-too-many", "x");
    assert_eq!(
        hub.admit(&extra, None, None).err(),
        Some(AdmitError::TooManyDevices)
    );
    // A listed device still comes back.
    assert!(hub.admit(&hello("d0", "x"), None, None).is_ok());

    let hub = Hub::new();
    let _phone = connect_phone(&hub, "cam1");
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    let values: Values = (0..MAX_STATE_KEYS + 10)
        .map(|i| (format!("k{i}"), Value::from(i)))
        .collect();
    hub.device_message(conn, "cam1", DeviceMessage::State { values });
    assert_eq!(hub.device_state("cam1").len(), MAX_STATE_KEYS);
}

#[test]
fn a_tally_that_did_not_fit_the_queue_is_sent_again() {
    let hub = Hub::new();
    let (tx, mut phone) = mpsc::channel(1);
    join(&hub, hub.new_connection_id(), hello("cam1", "Cam 1"), tx);
    // The join filled the one slot; the phone has not read it.
    let program = serde_json::json!({ "type": "tally", "device": "cam1", "state": "program" });
    controller(&hub, program.clone()).unwrap();
    assert_eq!(
        hub.devices()[0].tally,
        TallyState::Off,
        "not delivered, not recorded"
    );

    while phone.try_recv().is_ok() {}
    controller(&hub, program).unwrap();
    assert!(matches!(
        phone.try_recv(),
        Ok(ServerToDevice::Tally {
            state: TallyState::Program
        })
    ));
    assert_eq!(hub.devices()[0].tally, TallyState::Program);
}

#[test]
fn a_state_event_carries_only_the_kept_values() {
    let hub = Hub::new();
    let _phone = connect_phone(&hub, "cam1");
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    let mut events = hub.subscribe();
    let values: Values = (0..MAX_STATE_KEYS + 10)
        .map(|i| (format!("k{i:04}"), Value::from(i)))
        .collect();
    hub.device_message(conn, "cam1", DeviceMessage::State { values });
    let frame = events.try_recv().unwrap();
    let ControllerEvent::DeviceState { values, .. } = &*frame.event else {
        panic!("expected the state event")
    };
    assert_eq!(values.len(), MAX_STATE_KEYS);
}

#[test]
fn a_protocol_2_phone_is_asked_to_update() {
    let hub = Hub::new();
    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    let v2 = Hello {
        protocol: OWNERSHIP_PROTOCOL,
        ..hello("v2", "0.2.0 app")
    };
    join(&hub, hub.new_connection_id(), v2, tx);
    let mut texts = Vec::new();
    let mut production = false;
    while let Ok(msg) = phone.try_recv() {
        match msg {
            ServerToDevice::Message { text } => texts.push(text),
            ServerToDevice::Production(_) => production = true,
            _ => {}
        }
    }
    assert_eq!(texts, [OUTDATED_APP]);
    assert!(production, "still gets the production");

    // The current app is not.
    let (tx, mut phone) = mpsc::channel(DEVICE_QUEUE);
    join(&hub, hub.new_connection_id(), hello("v3", "new"), tx);
    while let Ok(msg) = phone.try_recv() {
        assert!(!matches!(msg, ServerToDevice::Message { .. }));
    }
}

// ---------------------------------------------------------------------------
// Device management: preview watch, tags, order, last seen
// ---------------------------------------------------------------------------

/// The preview commands a phone was sent since the last call (`enabled`
/// of each), skipping everything else.
fn previews(phone: &mut mpsc::Receiver<ServerToDevice>) -> Vec<bool> {
    std::iter::from_fn(|| phone.try_recv().ok())
        .filter_map(|m| match m {
            ServerToDevice::Preview { enabled, .. } => Some(enabled),
            _ => None,
        })
        .collect()
}

fn ids(list: &[&str]) -> Option<Vec<String>> {
    Some(list.iter().map(|s| s.to_string()).collect())
}

#[test]
fn a_controller_watching_one_device_gets_only_its_preview() {
    let hub = Arc::new(Hub::new());
    let mut a = connect_phone(&hub, "a");
    let mut b = connect_phone(&hub, "b");

    let (_, _rx, first) = hub.controller_joined();
    assert_eq!(previews(&mut a), [true]);
    assert_eq!(previews(&mut b), [true]);

    // Watching a: only b changes, and is switched off.
    first.watch(ids(&["a", "not-yet-known"]));
    assert_eq!(previews(&mut a), Vec::<bool>::new());
    assert_eq!(previews(&mut b), [false]);
    // The same watch again sends nothing.
    first.watch(ids(&["a"]));
    assert!(previews(&mut a).is_empty() && previews(&mut b).is_empty());

    // A device that joins now gets its preview off: nobody watches it.
    let mut c = connect_phone_raw(&hub, "c");
    assert_eq!(previews(&mut c), [false]);

    // A second controller that never sent watch wants every preview.
    let (_, _rx2, second) = hub.controller_joined();
    assert!(previews(&mut a).is_empty());
    assert_eq!(previews(&mut b), [true]);
    assert_eq!(previews(&mut c), [true]);

    // It leaves: back to the first controller's watch.
    drop(second);
    assert!(previews(&mut a).is_empty());
    assert_eq!(previews(&mut b), [false]);
    assert_eq!(previews(&mut c), [false]);

    // `null` is every device again.
    controller_watch(
        &first,
        serde_json::json!({ "type": "watch", "devices": null }),
    );
    assert!(previews(&mut a).is_empty());
    assert_eq!(previews(&mut b), [true]);
    assert_eq!(previews(&mut c), [true]);

    // The per-device preference still gates: switched off, a watch change
    // sends it nothing.
    controller(
        &hub,
        serde_json::json!({ "type": "preview", "device": "a", "enabled": false }),
    )
    .unwrap();
    assert_eq!(previews(&mut a), [false]);
    controller_watch(
        &first,
        serde_json::json!({ "type": "watch", "devices": ["b"] }),
    );
    assert!(previews(&mut a).is_empty());
    assert!(previews(&mut b).is_empty());
    assert_eq!(previews(&mut c), [false]);

    // The last controller leaves: only what was on goes off.
    drop(first);
    assert!(previews(&mut a).is_empty());
    assert_eq!(previews(&mut b), [false]);
    assert!(previews(&mut c).is_empty());
}

#[test]
fn image_stats_ignore_the_watch() {
    let hub = Arc::new(Hub::new());
    let mut a = connect_phone(&hub, "a");
    let (_, _rx, guard) = hub.controller_joined();
    guard.watch(Some(Vec::new()));
    let sent: Vec<_> = std::iter::from_fn(|| a.try_recv().ok()).collect();
    assert!(sent.contains(&ServerToDevice::Stats {
        enabled: true,
        fps: Some(DEFAULT_STATS_FPS)
    }));
    assert!(matches!(
        sent.last(),
        Some(ServerToDevice::Preview { enabled: false, .. })
    ));
}

/// A connected phone whose hello messages are left in the channel.
fn connect_phone_raw(hub: &Hub, id: &str) -> mpsc::Receiver<ServerToDevice> {
    let (tx, rx) = mpsc::channel(DEVICE_QUEUE);
    let conn = hub.new_connection_id();
    join(hub, conn, hello(id, id), tx);
    CONNS.with(|c| c.borrow_mut().insert(id.to_string(), conn));
    rx
}

/// A `watch` frame through the server's parsing.
fn controller_watch(guard: &ControllerGuard, json: Value) {
    match crate::protocol::ControllerInput::parse(&json.to_string()) {
        Ok(Some(crate::protocol::ControllerInput::Watch(devices))) => guard.watch(devices),
        other => panic!("not a watch: {other:?}"),
    }
}

#[test]
fn watch_tags_and_arrange_parse() {
    use crate::protocol::ControllerInput;
    let parse = |v: Value| ControllerInput::parse(&v.to_string());
    assert_eq!(
        parse(serde_json::json!({ "type": "watch" })),
        Ok(Some(ControllerInput::Watch(None)))
    );
    assert_eq!(
        parse(serde_json::json!({ "type": "watch", "devices": ["a"] })),
        Ok(Some(ControllerInput::Watch(ids(&["a"]))))
    );
    assert!(parse(serde_json::json!({ "type": "watch", "devices": [1] })).is_err());
    assert!(parse(serde_json::json!({ "type": "watch", "devices": "a" })).is_err());
    assert_eq!(
        parse(serde_json::json!({ "type": "tags", "device": "a", "tags": ["x"] })),
        Ok(Some(ControllerInput::Tags {
            device: "a".into(),
            tags: vec!["x".into()]
        }))
    );
    assert!(parse(serde_json::json!({ "type": "tags", "tags": ["x"] })).is_err());
    assert!(parse(serde_json::json!({ "type": "tags", "device": "a" })).is_err());
    assert_eq!(
        parse(serde_json::json!({ "type": "arrange", "order": ["b", "a"] })),
        Ok(Some(ControllerInput::Arrange(vec!["b".into(), "a".into()])))
    );
    assert!(parse(serde_json::json!({ "type": "arrange" })).is_err());
}

#[test]
fn tags_are_normalized() {
    let long = "é".repeat(MAX_TAG_CHARS + 5);
    let tags = normalize_tags(
        [
            "  Studio A ",
            "studio a",
            "",
            "   ",
            "Cr\u{7}owd\n",
            &long,
            "STUDIO A",
        ]
        .map(String::from),
    );
    assert_eq!(
        tags,
        ["Studio A", "Crowd", &"é".repeat(MAX_TAG_CHARS)].map(String::from)
    );
    let many = normalize_tags((0..MAX_TAGS + 4).map(|i| format!("t{i}")));
    assert_eq!(many.len(), MAX_TAGS);
    assert_eq!(many[MAX_TAGS - 1], format!("t{}", MAX_TAGS - 1));
}

#[test]
fn tags_are_kept_saved_and_sent() {
    let store = Arc::new(RecordingStore {
        initial: vec![StoredDevice {
            tags: vec!["Old".into(), "old".into()],
            ..stored("cam1", "Cam 1")
        }],
        ..Default::default()
    });
    let hub = Hub::with_store(store.clone());
    assert_eq!(hub.devices()[0].tags, ["Old"], "normalized when loaded");
    let mut events = hub.subscribe();

    controller(
        &hub,
        serde_json::json!({ "type": "tags", "device": "cam1", "tags": [" Studio A", "Crowd", "crowd"] }),
    )
    .unwrap();
    let want = vec!["Studio A".to_string(), "Crowd".to_string()];
    assert_eq!(
        *events.try_recv().unwrap().event,
        ControllerEvent::DeviceTags {
            device: "cam1".into(),
            tags: want.clone()
        }
    );
    assert_eq!(hub.devices()[0].tags, want);
    assert_eq!(store.saved.lock().unwrap().last().unwrap().tags, want);

    // Unchanged: no event, no save.
    let saves = store.saved.lock().unwrap().len();
    assert_eq!(hub.set_tags("cam1", want.clone()), Ok(want));
    assert!(events.try_recv().is_err());
    assert_eq!(store.saved.lock().unwrap().len(), saves);

    assert!(hub.set_tags("nope", vec!["x".into()]).is_err());

    // No tags: none in the snapshot's JSON.
    hub.set_tags("cam1", Vec::new()).unwrap();
    let json = serde_json::to_value(hub.devices()).unwrap();
    assert!(json[0].get("tags").is_none());
}

#[test]
fn arrange_sets_and_keeps_the_display_order() {
    let store = Arc::new(RecordingStore {
        initial: vec![stored("a", "A"), stored("b", "B"), stored("c", "C")],
        ..Default::default()
    });
    let hub = Hub::with_store(store.clone());
    let mut events = hub.subscribe();
    let order = |hub: &Hub| hub.devices().into_iter().map(|d| d.id).collect::<Vec<_>>();

    // Unknown ids ignored, duplicates once, the rest after in their order.
    controller(
        &hub,
        serde_json::json!({ "type": "arrange", "order": ["c", "x", "a", "c"] }),
    )
    .unwrap();
    assert_eq!(order(&hub), ["c", "a", "b"]);
    assert_eq!(
        *events.try_recv().unwrap().event,
        ControllerEvent::Order {
            devices: vec!["c".into(), "a".into(), "b".into()]
        }
    );
    assert_eq!(
        store.order.lock().unwrap().as_deref(),
        Some(&["c".to_string(), "a".into(), "b".into()][..])
    );

    // The same order again: nothing.
    hub.arrange(vec!["c".into()]);
    assert!(events.try_recv().is_err());

    // A new device goes last; snapshots follow the order.
    let _d = connect_phone(&hub, "d");
    assert_eq!(order(&hub), ["c", "a", "b", "d"]);
    let (snapshot, _rx) = hub.resync();
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
    let ids: Vec<_> = snapshot["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, ["c", "a", "b", "d"]);
}

#[test]
fn last_seen_is_set_on_hello_and_offline() {
    let store = Arc::new(RecordingStore {
        initial: vec![StoredDevice {
            last_seen_s: Some(1_000),
            ..stored("cam1", "Cam 1")
        }],
        ..Default::default()
    });
    let hub = Hub::with_store(store.clone());
    assert_eq!(hub.devices()[0].last_seen_s, Some(1_000));

    let _phone = connect_phone(&hub, "cam1");
    let seen = hub.devices()[0].last_seen_s.unwrap();
    assert!(seen > 1_000);
    assert_eq!(
        store.saved.lock().unwrap().last().unwrap().last_seen_s,
        Some(seen)
    );

    let mut events = hub.subscribe();
    let conn = CONNS.with(|c| c.borrow()["cam1"]);
    hub.device_disconnected(conn, "cam1");
    let frame = events.try_recv().unwrap();
    let ControllerEvent::DeviceOffline { last_seen_s, .. } = &*frame.event else {
        panic!("expected device_offline")
    };
    assert!(last_seen_s.is_some_and(|s| s >= seen));
    assert!(frame.json.contains(r#""last_seen_s":"#));
    assert_eq!(
        store.saved.lock().unwrap().last().unwrap().last_seen_s,
        *last_seen_s
    );
    assert_eq!(hub.devices()[0].last_seen_s, *last_seen_s);
}
