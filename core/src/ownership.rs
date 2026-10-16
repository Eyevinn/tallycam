//! Who owns a camera's exposure, colour and focus (docs/SHADING.md).
//!
//! Each group has one owner: `auto` (the phone's own auto exposure, AWB and
//! AF), `local` (the camera operator) or `remote` (the hub: a person at the
//! shader view, later the auto-shader). The phone runs this state machine
//! and reports it in `ownership`; the hub only forwards and displays it.
//!
//! The machine decides who may change a group. Applying values to the
//! camera (going back to auto on release, for one) stays with the platform:
//! the methods return what changed so the caller can act on it.
//!
//! Pure state machine; no I/O.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::protocol::{GroupOwnership, Holder, Owner, ShutterPolicy};

/// A group of controls with one owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Exposure,
    WhiteBalance,
    Focus,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Exposure, Group::WhiteBalance, Group::Focus];

    pub fn as_str(self) -> &'static str {
        match self {
            Group::Exposure => "exposure",
            Group::WhiteBalance => "white_balance",
            Group::Focus => "focus",
        }
    }

    pub fn parse(name: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.as_str() == name)
    }
}

/// The group a control belongs to, or None for controls outside ownership
/// (lens, zoom, stream settings). The shutter is a production setting and
/// joins the exposure group only under [`ShutterPolicy::Free`].
pub fn control_group(control: &str, shutter: ShutterPolicy) -> Option<Group> {
    match control {
        "exposure_mode" | "iso" | "ev_bias" => Some(Group::Exposure),
        "shutter" if shutter == ShutterPolicy::Free => Some(Group::Exposure),
        "wb_mode" | "wb_temperature" | "wb_tint" => Some(Group::WhiteBalance),
        "focus_mode" | "focus_position" => Some(Group::Focus),
        _ => None,
    }
}

/// The group a command belongs to. `focus_at` also moves the exposure
/// metering point, but only while exposure is auto, so it never changes the
/// exposure group's owner.
pub fn command_group(command: &str) -> Option<Group> {
    match command {
        "wb_one_shot" => Some(Group::WhiteBalance),
        "focus_at" => Some(Group::Focus),
        _ => None,
    }
}

/// What the phone shows for a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Indicator {
    Auto,
    Locked,
    Remote,
    RemoteDisconnected,
}

/// Why a change to a group was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The phone has no such group (a fixed-focus camera has no focus).
    NoSuchGroup(String),
    /// The remote may not change a group it does not own.
    NotRemote(Group),
    /// The camera operator may not change a remote group.
    RemoteOwned(Group),
    /// Taking a remote group local is only possible while disconnected.
    StillConnected(Group),
}

impl Refusal {
    /// Stable name, used by the test vectors.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NoSuchGroup(_) => "no_such_group",
            Refusal::NotRemote(_) => "not_remote",
            Refusal::RemoteOwned(_) => "remote_owned",
            Refusal::StillConnected(_) => "still_connected",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoSuchGroup(name) => write!(f, "{name}: no such group on this camera"),
            Refusal::NotRemote(g) => write!(f, "{}: not under remote control", g.as_str()),
            Refusal::RemoteOwned(g) => write!(f, "{}: under remote control", g.as_str()),
            Refusal::StillConnected(g) => {
                write!(f, "{}: under remote control and connected", g.as_str())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    owner: Owner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    holder: Option<Holder>,
}

impl Entry {
    fn auto() -> Self {
        Entry {
            owner: Owner::Auto,
            holder: None,
        }
    }
}

/// Outcome of [`Ownership::own`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Taken {
    /// Groups now owned by the new holder that were not before.
    pub taken: Vec<Group>,
    /// Of those, the ones the camera operator had (show a banner).
    pub from_local: Vec<Group>,
    /// Requested groups the phone does not have.
    pub skipped: Vec<String>,
}

/// Ownership of every group a camera has. Persist it with serde (D3): a
/// restored state starts disconnected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ownership {
    groups: BTreeMap<Group, Entry>,
    /// The control channel is up. Not persisted.
    #[serde(skip)]
    connected: bool,
}

impl Ownership {
    /// All `groups` in auto, disconnected.
    pub fn new(groups: impl IntoIterator<Item = Group>) -> Self {
        Ownership {
            groups: groups.into_iter().map(|g| (g, Entry::auto())).collect(),
            connected: false,
        }
    }

    /// The camera's groups changed (lens switch): new groups start in auto,
    /// gone ones are dropped, the rest keep their owner.
    pub fn set_groups(&mut self, groups: impl IntoIterator<Item = Group>) {
        let mut next = BTreeMap::new();
        for g in groups {
            let entry = self.groups.remove(&g).unwrap_or_else(Entry::auto);
            next.insert(g, entry);
        }
        self.groups = next;
    }

    pub fn has(&self, group: Group) -> bool {
        self.groups.contains_key(&group)
    }

    pub fn owner(&self, group: Group) -> Option<Owner> {
        self.groups.get(&group).map(|e| e.owner)
    }

    pub fn holder(&self, group: Group) -> Option<&Holder> {
        self.groups.get(&group).and_then(|e| e.holder.as_ref())
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn indicator(&self, group: Group) -> Option<Indicator> {
        self.groups.get(&group).map(|e| match e.owner {
            Owner::Auto => Indicator::Auto,
            Owner::Local => Indicator::Locked,
            Owner::Remote if self.connected => Indicator::Remote,
            Owner::Remote => Indicator::RemoteDisconnected,
        })
    }

    /// The `ownership` message body.
    pub fn report(&self) -> BTreeMap<String, GroupOwnership> {
        self.groups
            .iter()
            .map(|(g, e)| {
                (
                    g.as_str().to_string(),
                    GroupOwnership {
                        owner: e.owner,
                        holder: e.holder.clone(),
                    },
                )
            })
            .collect()
    }

    /// The control channel came up (admitted) or went down. Remote groups
    /// keep their values either way (rule 6).
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// `own` from the hub: the holder takes the named groups, from auto,
    /// local or another holder. Values are left as they are.
    pub fn own(&mut self, groups: &[String], holder: &Holder) -> Taken {
        let mut out = Taken::default();
        for name in groups {
            let Some(entry) =
                Group::parse(name).and_then(|g| self.groups.get_mut(&g).map(|e| (g, e)))
            else {
                out.skipped.push(name.clone());
                continue;
            };
            let (group, entry) = entry;
            if entry.owner == Owner::Remote && entry.holder.as_ref() == Some(holder) {
                continue;
            }
            if entry.owner == Owner::Local {
                out.from_local.push(group);
            }
            entry.owner = Owner::Remote;
            entry.holder = Some(holder.clone());
            out.taken.push(group);
        }
        out
    }

    /// `release` from the hub: the named remote groups go back to auto.
    /// Returns the groups the caller must put into auto on the camera.
    /// Releasing a group that is not remote does nothing.
    pub fn release(&mut self, groups: &[String]) -> Vec<Group> {
        let mut released = Vec::new();
        for g in groups.iter().filter_map(|n| Group::parse(n)) {
            if let Some(entry) = self.groups.get_mut(&g) {
                if entry.owner == Owner::Remote {
                    *entry = Entry::auto();
                    released.push(g);
                }
            }
        }
        released
    }

    /// May the remote change this group (a `set` key or a command)?
    pub fn check_remote(&self, group: Group) -> Result<(), Refusal> {
        match self.owner(group) {
            None => Err(Refusal::NoSuchGroup(group.as_str().into())),
            Some(Owner::Remote) => Ok(()),
            Some(_) => Err(Refusal::NotRemote(group)),
        }
    }

    /// The camera operator locked or set a value in a group: auto becomes
    /// local. Refused while the group is remote. Returns true on a change of
    /// owner.
    pub fn local_change(&mut self, group: Group) -> Result<bool, Refusal> {
        let entry = self
            .groups
            .get_mut(&group)
            .ok_or_else(|| Refusal::NoSuchGroup(group.as_str().into()))?;
        match entry.owner {
            Owner::Remote => Err(Refusal::RemoteOwned(group)),
            Owner::Local => Ok(false),
            Owner::Auto => {
                entry.owner = Owner::Local;
                Ok(true)
            }
        }
    }

    /// The camera operator chose auto: local becomes auto. Refused while
    /// the group is remote. Returns true on a change of owner.
    pub fn local_auto(&mut self, group: Group) -> Result<bool, Refusal> {
        let entry = self
            .groups
            .get_mut(&group)
            .ok_or_else(|| Refusal::NoSuchGroup(group.as_str().into()))?;
        match entry.owner {
            Owner::Remote => Err(Refusal::RemoteOwned(group)),
            Owner::Auto => Ok(false),
            Owner::Local => {
                *entry = Entry::auto();
                Ok(true)
            }
        }
    }

    /// The escape hatch (D2): the camera operator takes a remote group
    /// local, only while the hub is not connected. The values stay.
    pub fn take_local(&mut self, group: Group) -> Result<bool, Refusal> {
        let connected = self.connected;
        let entry = self
            .groups
            .get_mut(&group)
            .ok_or_else(|| Refusal::NoSuchGroup(group.as_str().into()))?;
        match entry.owner {
            Owner::Remote if connected => Err(Refusal::StillConnected(group)),
            Owner::Remote => {
                entry.owner = Owner::Local;
                entry.holder = None;
                Ok(true)
            }
            Owner::Local => Ok(false),
            Owner::Auto => {
                entry.owner = Owner::Local;
                Ok(true)
            }
        }
    }

    /// A new control server was provisioned (rule 8): every remote group
    /// goes back to auto. Returns the groups to put into auto.
    pub fn new_server(&mut self) -> Vec<Group> {
        let remote: Vec<String> = self
            .groups
            .iter()
            .filter(|(_, e)| e.owner == Owner::Remote)
            .map(|(g, _)| g.as_str().to_string())
            .collect();
        self.release(&remote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(id: &str) -> Holder {
        Holder {
            id: id.into(),
            label: format!("Shader {id}"),
        }
    }

    fn names(groups: &[Group]) -> Vec<String> {
        groups.iter().map(|g| g.as_str().to_string()).collect()
    }

    #[test]
    fn remote_holds_values_through_a_lost_link() {
        let mut o = Ownership::new(Group::ALL);
        o.set_connected(true);
        o.own(&names(&[Group::Exposure]), &holder("a"));
        o.set_connected(false);
        assert_eq!(
            o.indicator(Group::Exposure),
            Some(Indicator::RemoteDisconnected)
        );
        assert_eq!(o.owner(Group::Exposure), Some(Owner::Remote));
        assert_eq!(
            o.local_change(Group::Exposure),
            Err(Refusal::RemoteOwned(Group::Exposure))
        );
        o.set_connected(true);
        assert_eq!(o.indicator(Group::Exposure), Some(Indicator::Remote));
    }

    #[test]
    fn persisted_state_restores_disconnected() {
        let mut o = Ownership::new(Group::ALL);
        o.set_connected(true);
        o.own(&names(&[Group::Focus]), &holder("a"));
        let saved = serde_json::to_string(&o).unwrap();
        let back: Ownership = serde_json::from_str(&saved).unwrap();
        assert!(!back.connected());
        assert_eq!(back.holder(Group::Focus), Some(&holder("a")));
        assert_eq!(
            back.indicator(Group::Focus),
            Some(Indicator::RemoteDisconnected)
        );
    }

    #[test]
    fn shutter_joins_exposure_only_when_free() {
        assert_eq!(control_group("shutter", ShutterPolicy::Mains), None);
        assert_eq!(control_group("shutter", ShutterPolicy::Fixed(100)), None);
        assert_eq!(
            control_group("shutter", ShutterPolicy::Free),
            Some(Group::Exposure)
        );
        assert_eq!(control_group("zoom", ShutterPolicy::Free), None);
    }

    #[test]
    fn lens_switch_keeps_owners_of_remaining_groups() {
        let mut o = Ownership::new(Group::ALL);
        o.own(&names(&[Group::Exposure, Group::Focus]), &holder("a"));
        o.set_groups([Group::Exposure, Group::WhiteBalance]);
        assert_eq!(o.owner(Group::Exposure), Some(Owner::Remote));
        assert!(!o.has(Group::Focus));
        o.set_groups(Group::ALL);
        assert_eq!(o.owner(Group::Focus), Some(Owner::Auto));
    }
}
