//! Shading for Swift (docs/SHADING.md): the ownership state machine, the
//! group of a control, and the image statistics. UniFFI; none of it is per
//! frame (statistics run a few times a second at most).

use std::sync::Mutex;

use tallycam_core::image_stats::{self, ExposureWarning, Grid};
use tallycam_core::ownership::{self as core, Group, Ownership as Machine, Refusal};
use tallycam_core::protocol::{Holder, Owner, ShutterPolicy};

/// The result of a change asked of the machine.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OwnershipChange {
    Changed,
    Unchanged,
    /// Refused, with a message for the operator or the server.
    Refused { reason: String },
}

impl From<Result<bool, Refusal>> for OwnershipChange {
    fn from(r: Result<bool, Refusal>) -> Self {
        match r {
            Ok(true) => Self::Changed,
            Ok(false) => Self::Unchanged,
            Err(e) => Self::Refused {
                reason: e.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OwnTaken {
    pub taken: Vec<String>,
    pub from_local: Vec<String>,
    pub skipped: Vec<String>,
}

/// One group as the phone shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GroupView {
    pub group: String,
    /// `auto`, `locked`, `remote` or `remote_disconnected`.
    pub indicator: String,
    pub holder_label: Option<String>,
}

fn groups(names: &[String]) -> impl Iterator<Item = Group> + '_ {
    names.iter().filter_map(|n| Group::parse(n))
}

fn names(groups: &[Group]) -> Vec<String> {
    groups.iter().map(|g| g.as_str().to_string()).collect()
}

/// The ownership state machine of one camera. Calls are serialized by the
/// mutex; the app calls it from the main actor.
#[derive(uniffi::Object)]
pub struct OwnershipMachine(Mutex<Machine>);

impl OwnershipMachine {
    fn lock(&self) -> std::sync::MutexGuard<'_, Machine> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[uniffi::export]
impl OwnershipMachine {
    /// All `groups` in auto, or the state saved with `save_json` (restored
    /// disconnected; unknown or broken saves start in auto).
    #[uniffi::constructor]
    pub fn new(groups: Vec<String>, saved_json: Option<String>) -> Self {
        let mut machine = saved_json
            .and_then(|s| serde_json::from_str::<Machine>(&s).ok())
            .unwrap_or_else(|| Machine::new(self::groups(&groups)));
        machine.set_groups(self::groups(&groups));
        Self(Mutex::new(machine))
    }

    /// For persisting across restarts (D3).
    pub fn save_json(&self) -> String {
        serde_json::to_string(&*self.lock()).unwrap_or_default()
    }

    pub fn set_groups(&self, groups: Vec<String>) {
        self.lock().set_groups(self::groups(&groups));
    }

    pub fn set_connected(&self, connected: bool) {
        self.lock().set_connected(connected);
    }

    pub fn own(&self, groups: Vec<String>, holder_id: String, holder_label: String) -> OwnTaken {
        let taken = self.lock().own(
            &groups,
            &Holder {
                id: holder_id,
                label: holder_label,
            },
        );
        OwnTaken {
            taken: names(&taken.taken),
            from_local: names(&taken.from_local),
            skipped: taken.skipped,
        }
    }

    /// Returns the groups to put into auto on the camera.
    pub fn release(&self, groups: Vec<String>) -> Vec<String> {
        names(&self.lock().release(&groups))
    }

    /// Returns the groups to put into auto on the camera.
    pub fn new_server(&self) -> Vec<String> {
        names(&self.lock().new_server())
    }

    /// None when the remote may change `group`, else the reason.
    pub fn check_remote(&self, group: String) -> Option<String> {
        let Some(g) = Group::parse(&group) else {
            return Some(format!("{group}: no such group on this camera"));
        };
        self.lock().check_remote(g).err().map(|e| e.to_string())
    }

    pub fn local_change(&self, group: String) -> OwnershipChange {
        match Group::parse(&group) {
            Some(g) => self.lock().local_change(g).into(),
            None => no_such_group(group),
        }
    }

    pub fn local_auto(&self, group: String) -> OwnershipChange {
        match Group::parse(&group) {
            Some(g) => self.lock().local_auto(g).into(),
            None => no_such_group(group),
        }
    }

    pub fn take_local(&self, group: String) -> OwnershipChange {
        match Group::parse(&group) {
            Some(g) => self.lock().take_local(g).into(),
            None => no_such_group(group),
        }
    }

    /// Every group the camera has, in a fixed order.
    pub fn view(&self) -> Vec<GroupView> {
        let m = self.lock();
        Group::ALL
            .into_iter()
            .filter_map(|g| {
                let indicator = m.indicator(g)?;
                Some(GroupView {
                    group: g.as_str().into(),
                    indicator: serde_json::to_value(indicator)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default(),
                    holder_label: (m.owner(g) == Some(Owner::Remote))
                        .then(|| m.holder(g).map(|h| h.label.clone()))
                        .flatten(),
                })
            })
            .collect()
    }

    /// The `ownership` message, as JSON.
    pub fn message_json(&self) -> String {
        serde_json::json!({ "type": "ownership", "groups": self.lock().report() }).to_string()
    }
}

fn no_such_group(group: String) -> OwnershipChange {
    Err::<bool, _>(Refusal::NoSuchGroup(group)).into()
}

/// The group of a control (`exposure`, `white_balance`, `focus`), or None.
#[uniffi::export]
pub fn shading_control_group(control: String, shutter_free: bool) -> Option<String> {
    let shutter = if shutter_free {
        ShutterPolicy::Free
    } else {
        ShutterPolicy::Mains
    };
    core::control_group(&control, shutter).map(|g| g.as_str().into())
}

/// The group of a command, or None.
#[uniffi::export]
pub fn shading_command_group(command: String) -> Option<String> {
    core::command_group(&command).map(|g| g.as_str().into())
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImageStatsResult {
    /// `{"luma": ..., "colour": ...}`, to merge into `image_stats`.
    pub json: String,
    pub mean: f64,
    pub clipped: f64,
    pub crushed: f64,
    /// `too_dark`, `clipping`, or None.
    pub warning: Option<String>,
}

/// Statistics of a sampled grid (`width * height` luma bytes and as many
/// Cb/Cr pairs, or no chroma). None for an inconsistent grid.
#[uniffi::export]
pub fn image_stats_compute(
    y: Vec<u8>,
    cbcr: Vec<u8>,
    width: u32,
    height: u32,
    video_range: bool,
) -> Option<ImageStatsResult> {
    let stats = image_stats::compute(&Grid {
        y: &y,
        cbcr: &cbcr,
        width: width as usize,
        height: height as usize,
        video_range,
    })?;
    let warning = image_stats::exposure_warning(&stats.luma).map(|w| match w {
        ExposureWarning::TooDark => "too_dark".to_string(),
        ExposureWarning::Clipping => "clipping".to_string(),
    });
    Some(ImageStatsResult {
        mean: stats.luma.mean,
        clipped: stats.luma.clipped,
        crushed: stats.luma.crushed,
        warning,
        json: serde_json::to_string(&stats).unwrap_or_default(),
    })
}
