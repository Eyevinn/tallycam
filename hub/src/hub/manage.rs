//! Device management for many cameras: which previews each controller
//! watches, the operator's tags and the display order.

use super::*;

/// Tags per device, at most; more are dropped.
pub const MAX_TAGS: usize = 16;
/// Characters per tag, at most; a longer tag is cut.
pub const MAX_TAG_CHARS: usize = 40;

/// Tags as the hub keeps them: control characters removed, trimmed, cut to
/// [`MAX_TAG_CHARS`] characters, empty ones dropped, duplicates (ignoring
/// case) dropped keeping the first spelling, at most [`MAX_TAGS`].
pub fn normalize_tags(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for tag in tags {
        if out.len() == MAX_TAGS {
            break;
        }
        let clean: String = tag.chars().filter(|c| !c.is_control()).collect();
        let cut: String = clean.trim().chars().take(MAX_TAG_CHARS).collect();
        let cut = cut.trim_end();
        if cut.is_empty() || !seen.insert(cut.to_lowercase()) {
            continue;
        }
        out.push(cut.to_string());
    }
    out
}

impl Inner {
    /// Does a connected controller show `device`'s preview: one that never
    /// sent `watch`, or one whose `watch` lists it?
    pub(super) fn watched(&self, device: &str) -> bool {
        self.controllers
            .values()
            .any(|w| w.as_ref().is_none_or(|set| set.contains(device)))
    }
}

impl Hub {
    /// Change the controller registry with `change`, then send a preview
    /// command to each device whose preview went on or off with it (and
    /// only to those).
    pub(super) fn update_watch(
        inner: &mut Inner,
        change: impl FnOnce(&mut HashMap<u64, Option<HashSet<String>>>),
    ) {
        let before: Vec<bool> = inner.order.iter().map(|id| inner.watched(id)).collect();
        change(&mut inner.controllers);
        for (id, was) in inner.order.iter().zip(before) {
            let now = inner.watched(id);
            if now == was {
                continue;
            }
            if let Some(entry) = inner.devices.get(id) {
                // With previews switched off for the device, nothing changes.
                if entry.preview_pref.enabled {
                    entry.send(entry.preview_command(now));
                }
            }
        }
    }

    /// Controller `controller`'s `watch` ([`ControllerGuard::watch`]).
    /// Unknown ids are kept (the device may say hello later), at most
    /// [`MAX_DEVICES`] of them.
    pub(super) fn watch(&self, controller: u64, devices: Option<Vec<String>>) {
        let set = devices.map(|d| d.into_iter().take(MAX_DEVICES).collect::<HashSet<_>>());
        let mut inner = self.lock();
        if !inner.controllers.contains_key(&controller) {
            return; // already gone
        }
        match &set {
            Some(s) => debug!("Controller {controller} watches {} devices", s.len()),
            None => debug!("Controller {controller} watches every device"),
        }
        Self::update_watch(&mut inner, |c| {
            c.insert(controller, set);
        });
    }

    /// Set a device's tags, from a controller (`tags`) or the embedding
    /// server. They are normalized ([`normalize_tags`]), kept in the store
    /// and sent to every controller (`device_tags`) when they changed.
    /// Returns the tags as kept.
    pub fn set_tags(&self, device: &str, tags: Vec<String>) -> Result<Vec<String>, String> {
        let tags = normalize_tags(tags);
        let mut inner = self.lock();
        let Some(entry) = inner.devices.get_mut(device) else {
            return Err(format!("unknown device '{device}'"));
        };
        if entry.tags != tags {
            entry.tags = tags.clone();
            self.store.save(&entry.stored());
            self.emit(ControllerEvent::DeviceTags {
                device: device.to_string(),
                tags: tags.clone(),
            });
        }
        Ok(tags)
    }

    /// Set the display order, from a controller (`arrange`) or the embedding
    /// server: the listed devices first, in that order, then those not
    /// listed in their old order. Unknown ids are ignored. Kept in the
    /// store ([`DeviceStore::save_order`]) and sent to every controller
    /// (`order`) when it changed.
    pub fn arrange(&self, order: Vec<String>) {
        let mut inner = self.lock();
        let mut listed = HashSet::new();
        let mut next: Vec<String> = order
            .into_iter()
            .filter(|id| inner.devices.contains_key(id) && listed.insert(id.clone()))
            .collect();
        next.extend(
            inner
                .order
                .iter()
                .filter(|id| !listed.contains(*id))
                .cloned(),
        );
        if next == inner.order {
            return;
        }
        inner.order = next;
        self.store.save_order(&inner.order);
        self.emit(ControllerEvent::Order {
            devices: inner.order.clone(),
        });
    }
}
