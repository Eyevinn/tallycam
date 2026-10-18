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

/// The query of an SRT URL: everything after the first `?`. An SRT URL has
/// no fragment, and a raw `#` is common in stream ids (`#!::r=cam1`), so it
/// does not end the query.
fn query(url: &str) -> Option<(usize, &str)> {
    let start = url.find('?')? + 1;
    Some((start, &url[start..]))
}

/// The byte ranges of every `name` query value in `url` (the phone uses the
/// last one).
fn param_ranges(url: &str, name: &str) -> Vec<std::ops::Range<usize>> {
    let Some((mut pos, query)) = query(url) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for part in query.split('&') {
        if let Some((key, _)) = part.split_once('=') {
            if key.eq_ignore_ascii_case(name) {
                out.push(pos + key.len() + 1..pos + part.len());
            }
        }
        pos += part.len() + 1;
    }
    out
}

/// The last `name` value in `url`.
fn param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    param_ranges(url, name).pop().map(|r| &url[r])
}

/// Scheme and authority (`srt://host:port`), lowercase: where the phone
/// sends the picture.
fn target(url: &str) -> String {
    let end = url
        .find("://")
        .map(|i| i + 3)
        .and_then(|i| url[i..].find(['/', '?']).map(|j| i + j))
        .unwrap_or(url.len());
    url[..end].to_ascii_lowercase()
}

/// `url` with every passphrase shown as `***`.
pub(super) fn hide(url: &str) -> Option<String> {
    let ranges: Vec<_> = param_ranges(url, "passphrase")
        .into_iter()
        .filter(|r| !r.is_empty() && &url[r.clone()] != HIDDEN)
        .collect();
    if ranges.is_empty() {
        return None;
    }
    let mut out = url.to_string();
    for r in ranges.into_iter().rev() {
        out.replace_range(r, HIDDEN);
    }
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
/// passphrase of `current` (the phone's value) back, but only for the same
/// target and stream id. A URL to another listener with `***` would hand
/// that listener the passphrase's key material, so it is refused: the
/// controller sends the new passphrase itself.
pub(super) fn restore(values: &mut Values, current: &Values) -> Result<(), String> {
    let Some(Value::String(url)) = values.get_mut(STREAM_URL) else {
        return Ok(());
    };
    let hidden: Vec<_> = param_ranges(url, "passphrase")
        .into_iter()
        .filter(|r| &url[r.clone()] == HIDDEN)
        .collect();
    if hidden.is_empty() {
        return Ok(());
    }
    let refuse = || {
        Err("stream_url: the hidden passphrase is kept only for the same SRT target and stream id; give the passphrase for a new one".to_string())
    };
    let Some(Value::String(old)) = current.get(STREAM_URL) else {
        return refuse();
    };
    let same = target(url) == target(old) && param(url, "streamid") == param(old, "streamid");
    let Some(secret) = param(old, "passphrase").filter(|p| same && !p.is_empty()) else {
        return refuse();
    };
    let secret = secret.to_string();
    for r in hidden.into_iter().rev() {
        url.replace_range(r, &secret);
    }
    Ok(())
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
        let v = values("srt://h:9000?streamid=cam1&PassPhrase=s3cret%26pass&pbkeylen=32");
        assert_eq!(
            url(&for_controllers(&v)),
            "srt://h:9000?streamid=cam1&PassPhrase=***&pbkeylen=32"
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
        let mut set = values("SRT://H:9000?passphrase=***&latency=200");
        restore(&mut set, &current).unwrap();
        assert_eq!(url(&set), "SRT://H:9000?passphrase=0123456789&latency=200");

        // A new passphrase is sent as given.
        let mut new = values("srt://other:9000?passphrase=abcdefghijk");
        restore(&mut new, &current).unwrap();
        assert_eq!(url(&new), "srt://other:9000?passphrase=abcdefghijk");

        // Nothing to restore from: refused.
        let mut orphan = values("srt://h:9000?passphrase=***");
        assert!(restore(&mut orphan, &Values::new()).is_err());
    }

    #[test]
    fn the_passphrase_never_goes_to_another_listener() {
        let current = values("srt://strom.lan:9000?streamid=cam1&passphrase=RealSecret12345");
        for elsewhere in [
            "srt://203.0.113.66:9000?streamid=cam1&passphrase=***",
            "srt://strom.lan:9001?streamid=cam1&passphrase=***",
            "srt://strom.lan:9000?streamid=cam2&passphrase=***",
        ] {
            let mut set = values(elsewhere);
            assert!(restore(&mut set, &current).is_err(), "{elsewhere}");
            assert!(!url(&set).contains("RealSecret"), "{elsewhere}");
        }
        // Every passphrase is hidden and restored, the last one counts.
        let two = values("srt://h:9000?passphrase=first-secret&passphrase=last-secret");
        assert_eq!(
            url(&for_controllers(&two)),
            "srt://h:9000?passphrase=***&passphrase=***"
        );
        let mut set = values("srt://h:9000?passphrase=***&passphrase=***");
        restore(&mut set, &two).unwrap();
        assert_eq!(
            url(&set),
            "srt://h:9000?passphrase=last-secret&passphrase=last-secret"
        );
    }

    #[test]
    fn a_raw_hash_in_the_stream_id_does_not_end_the_query() {
        let v = values("srt://h:9000?streamid=#!::r=cam1&passphrase=secret12345");
        assert_eq!(
            url(&for_controllers(&v)),
            "srt://h:9000?streamid=#!::r=cam1&passphrase=***"
        );
    }
}
