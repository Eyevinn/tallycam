//! The SRT passphrase stays between the hub and the phone.
//!
//! A phone's `stream_url` value carries the passphrase (docs/PROTOCOL.md).
//! The hub keeps the value as the phone sent it, but controllers (the
//! operator page, Strom's API) see `passphrase=***`. A controller that sends
//! such a URL back, say after editing the latency, keeps the phone's current
//! passphrase.

use serde_json::Value;

use crate::protocol::Values;

/// The control whose value is an SRT URL.
const STREAM_URL: &str = "stream_url";
/// What a passphrase reads as outside the hub.
const HIDDEN: &str = "***";

/// The byte range of the `passphrase` query value in `url`, if it has one.
fn passphrase_range(url: &str) -> Option<std::ops::Range<usize>> {
    let query_start = url.find('?')? + 1;
    let query_end = url[query_start..]
        .find('#')
        .map_or(url.len(), |i| query_start + i);
    let mut pos = query_start;
    for part in url[query_start..query_end].split('&') {
        if let Some((name, _)) = part.split_once('=') {
            if name.eq_ignore_ascii_case("passphrase") {
                let start = pos + name.len() + 1;
                return Some(start..pos + part.len());
            }
        }
        pos += part.len() + 1;
    }
    None
}

/// `url` with its passphrase shown as `***`.
fn hide(url: &str) -> Option<String> {
    let range = passphrase_range(url)?;
    if range.is_empty() || &url[range.clone()] == HIDDEN {
        return None;
    }
    let mut out = url.to_string();
    out.replace_range(range, HIDDEN);
    Some(out)
}

/// `values` as controllers may see them.
pub(super) fn for_controllers(values: &Values) -> Values {
    let mut out = values.clone();
    if let Some(Value::String(url)) = out.get_mut(STREAM_URL) {
        if let Some(hidden) = hide(url) {
            *url = hidden;
        }
    }
    out
}

/// A controller's `set`: a `stream_url` whose passphrase is `***` gets the
/// passphrase of `current` (the phone's value) back.
pub(super) fn restore(values: &mut Values, current: &Values) {
    let Some(Value::String(url)) = values.get_mut(STREAM_URL) else {
        return;
    };
    let Some(range) = passphrase_range(url) else {
        return;
    };
    if &url[range.clone()] != HIDDEN {
        return;
    }
    let Some(Value::String(old)) = current.get(STREAM_URL) else {
        return;
    };
    if let Some(old_range) = passphrase_range(old) {
        url.replace_range(range, &old[old_range]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(url: &str) -> Values {
        let mut v = Values::new();
        v.insert(STREAM_URL.into(), Value::String(url.into()));
        v
    }

    fn url(v: &Values) -> &str {
        v[STREAM_URL].as_str().unwrap()
    }

    #[test]
    fn hides_the_passphrase_only() {
        let v = values("srt://h:9000?streamid=cam1&PassPhrase=s3cret%26pass&pbkeylen=32#x");
        assert_eq!(
            url(&for_controllers(&v)),
            "srt://h:9000?streamid=cam1&PassPhrase=***&pbkeylen=32#x"
        );
        let last = values("srt://h:9000?latency=100&passphrase=0123456789");
        assert_eq!(
            url(&for_controllers(&last)),
            "srt://h:9000?latency=100&passphrase=***"
        );
        for plain in [
            "srt://h:9000",
            "srt://h:9000?streamid=passphrase",
            "srt://h?passphrase=",
        ] {
            assert_eq!(url(&for_controllers(&values(plain))), plain);
        }
    }

    #[test]
    fn a_hidden_passphrase_sent_back_is_restored() {
        let current = values("srt://h:9000?passphrase=0123456789&latency=100");
        let mut set = values("srt://h:9000?passphrase=***&latency=200");
        restore(&mut set, &current);
        assert_eq!(url(&set), "srt://h:9000?passphrase=0123456789&latency=200");

        // A new passphrase is sent as given.
        let mut new = values("srt://h:9000?passphrase=abcdefghijk");
        restore(&mut new, &current);
        assert_eq!(url(&new), "srt://h:9000?passphrase=abcdefghijk");

        // Nothing to restore from: left as is, and the phone refuses it.
        let mut orphan = values("srt://h:9000?passphrase=***");
        restore(&mut orphan, &Values::new());
        assert_eq!(url(&orphan), "srt://h:9000?passphrase=***");
    }
}
