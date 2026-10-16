//! Typed calls for an embedding server: `set` and `command` with an awaited
//! result, and operator messages, without building protocol JSON.

use super::*;

/// Why a typed call ([`Hub::set`], [`Hub::command`], [`Hub::message`]) did
/// not get a successful result.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// No device with that id has ever said hello (or been stored).
    UnknownDevice(String),
    /// The device is known but not connected.
    Offline(String),
    /// The connection closed before the phone answered.
    Disconnected(String),
    /// The phone did not answer within the timeout.
    Timeout(String),
    /// The phone answered `ok: false`.
    Failed {
        device: String,
        error: Option<String>,
    },
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::UnknownDevice(d) => write!(f, "unknown device '{d}'"),
            CallError::Offline(d) => write!(f, "device '{d}' is offline"),
            CallError::Disconnected(d) => write!(f, "device '{d}' disconnected before answering"),
            CallError::Timeout(d) => write!(f, "device '{d}' did not answer in time"),
            CallError::Failed { device, error } => write!(
                f,
                "device '{device}' refused: {}",
                error.as_deref().unwrap_or("no reason given")
            ),
        }
    }
}

impl std::error::Error for CallError {}

/// A `set` or `command` the hub sent and is waiting on.
pub(super) struct PendingCall {
    pub(super) device: String,
    pub(super) tx: oneshot::Sender<CommandResult>,
}

/// Removes a call from the pending map when the caller stops waiting
/// (answered, timed out or dropped).
struct PendingGuard<'a> {
    hub: &'a Hub,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.hub.lock().pending.remove(&self.id);
    }
}

impl Hub {
    // -----------------------------------------------------------------------
    // Typed calls, for an embedding server that does not build protocol JSON
    // -----------------------------------------------------------------------

    /// Set control values on a phone and wait for its `result`. `Ok` carries
    /// the phone's result (`ok: true`); a refusal is [`CallError::Failed`].
    pub async fn set(
        &self,
        device: &str,
        values: Values,
        timeout: Duration,
    ) -> Result<CommandResult, CallError> {
        self.call(device, timeout, |id| ServerToDevice::Set {
            id: Some(id),
            values,
        })
        .await
    }

    /// Run a command on a phone and wait for its `result` (output in `data`).
    pub async fn command(
        &self,
        device: &str,
        name: &str,
        args: Option<Map<String, Value>>,
        timeout: Duration,
    ) -> Result<CommandResult, CallError> {
        let name = name.to_string();
        self.call(device, timeout, |id| ServerToDevice::Command {
            id: Some(id),
            name,
            args,
        })
        .await
    }

    /// Show an operator message on a phone. The protocol has no reply for
    /// it, so `Ok` means it was handed to the connection.
    pub fn message(&self, device: &str, text: &str) -> Result<(), CallError> {
        let inner = self.lock();
        let entry = inner
            .devices
            .get(device)
            .ok_or_else(|| CallError::UnknownDevice(device.to_string()))?;
        if entry.conn.is_none() {
            return Err(CallError::Offline(device.to_string()));
        }
        if !entry.send(ServerToDevice::Message {
            text: text.to_string(),
        }) {
            return Err(CallError::Disconnected(device.to_string()));
        }
        Ok(())
    }

    async fn call(
        &self,
        device: &str,
        timeout: Duration,
        make: impl FnOnce(Value) -> ServerToDevice,
    ) -> Result<CommandResult, CallError> {
        let id = format!(
            "{CALL_ID_PREFIX}{}",
            self.next_call.fetch_add(1, Ordering::Relaxed)
        );
        let rx = {
            let mut inner = self.lock();
            let entry = inner
                .devices
                .get(device)
                .ok_or_else(|| CallError::UnknownDevice(device.to_string()))?;
            if entry.conn.is_none() {
                return Err(CallError::Offline(device.to_string()));
            }
            if !entry.send(make(Value::String(id.clone()))) {
                return Err(CallError::Disconnected(device.to_string()));
            }
            let (tx, rx) = oneshot::channel();
            inner.pending.insert(
                id.clone(),
                PendingCall {
                    device: device.to_string(),
                    tx,
                },
            );
            rx
        };
        let _guard = PendingGuard { hub: self, id };
        match tokio::time::timeout(timeout, rx).await {
            Err(_) => Err(CallError::Timeout(device.to_string())),
            Ok(Err(_)) => Err(CallError::Disconnected(device.to_string())),
            Ok(Ok(result)) if result.ok => Ok(result),
            Ok(Ok(result)) => Err(CallError::Failed {
                device: device.to_string(),
                error: result.error,
            }),
        }
    }
}
