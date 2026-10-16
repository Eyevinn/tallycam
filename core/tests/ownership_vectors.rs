//! The shared ownership vectors (`tests/vectors/lcp_ownership.json`).

use serde::Deserialize;
use serde_json::{Map, Value};
use tallycam_core::ownership::{Group, Ownership, Refusal};
use tallycam_core::protocol::{DeviceMessage, Holder, ServerToDevice};

#[derive(Deserialize)]
struct MessageVector {
    name: String,
    direction: String,
    json: Value,
    canonical: Value,
}

#[derive(Deserialize)]
struct Step {
    #[serde(rename = "do")]
    action: String,
    group: Option<String>,
    groups: Option<Vec<String>>,
    holder: Option<Holder>,
    result: Option<String>,
    taken: Option<Vec<String>>,
    from_local: Option<Vec<String>>,
    skipped: Option<Vec<String>>,
    released: Option<Vec<String>>,
    ownership: Option<Value>,
    indicators: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    groups: Vec<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct File {
    messages: Vec<MessageVector>,
    scenarios: Vec<Scenario>,
}

fn file() -> File {
    serde_json::from_str(include_str!("vectors/lcp_ownership.json")).unwrap()
}

fn group(name: &str) -> Group {
    Group::parse(name).unwrap_or_else(|| panic!("unknown group {name} in a vector"))
}

fn names(groups: &[Group]) -> Vec<String> {
    groups.iter().map(|g| g.as_str().to_string()).collect()
}

fn outcome(result: Result<bool, Refusal>) -> String {
    match result {
        Ok(true) => "changed".into(),
        Ok(false) => "unchanged".into(),
        Err(refusal) => refusal.code().into(),
    }
}

#[test]
fn messages_round_trip() {
    let file = file();
    assert!(file.messages.len() >= 8);
    for v in file.messages {
        let back = match v.direction.as_str() {
            "server_to_device" => {
                let msg: ServerToDevice = serde_json::from_value(v.json.clone())
                    .unwrap_or_else(|e| panic!("{}: {e}", v.name));
                serde_json::to_value(&msg).unwrap()
            }
            "device" => {
                let msg: DeviceMessage = serde_json::from_value(v.json.clone())
                    .unwrap_or_else(|e| panic!("{}: {e}", v.name));
                assert_ne!(msg, DeviceMessage::Unknown, "{}", v.name);
                serde_json::to_value(&msg).unwrap()
            }
            other => panic!("{}: unknown direction {other}", v.name),
        };
        assert_eq!(back, v.canonical, "{}", v.name);
    }
}

#[test]
fn scenarios() {
    let file = file();
    assert!(file.scenarios.len() >= 10);
    for s in file.scenarios {
        let mut o = Ownership::new(s.groups.iter().map(|g| group(g)));
        for (i, step) in s.steps.iter().enumerate() {
            let at = format!("{} step {i} ({})", s.name, step.action);
            let one = || group(step.group.as_deref().expect("step needs 'group'"));
            let many = || step.groups.clone().expect("step needs 'groups'");
            let result = match step.action.as_str() {
                "connect" => {
                    o.set_connected(true);
                    None
                }
                "disconnect" => {
                    o.set_connected(false);
                    None
                }
                "restart" => {
                    let saved = serde_json::to_string(&o).unwrap();
                    o = serde_json::from_str(&saved).unwrap();
                    None
                }
                "own" => {
                    let taken = o.own(&many(), step.holder.as_ref().expect("own needs 'holder'"));
                    if let Some(want) = &step.taken {
                        assert_eq!(&names(&taken.taken), want, "{at}: taken");
                    }
                    if let Some(want) = &step.from_local {
                        assert_eq!(&names(&taken.from_local), want, "{at}: from_local");
                    }
                    if let Some(want) = &step.skipped {
                        assert_eq!(&taken.skipped, want, "{at}: skipped");
                    }
                    None
                }
                "release" => {
                    let released = o.release(&many());
                    if let Some(want) = &step.released {
                        assert_eq!(&names(&released), want, "{at}: released");
                    }
                    None
                }
                "new_server" => {
                    let released = o.new_server();
                    if let Some(want) = &step.released {
                        assert_eq!(&names(&released), want, "{at}: released");
                    }
                    None
                }
                "remote_set" => {
                    // A group name the vectors use for a missing group may
                    // still be a known group the camera lacks.
                    Some(outcome(o.check_remote(one()).map(|()| true)))
                }
                "local_change" => Some(outcome(o.local_change(one()))),
                "local_auto" => Some(outcome(o.local_auto(one()))),
                "take_local" => Some(outcome(o.take_local(one()))),
                other => panic!("{at}: unknown action {other}"),
            };
            if let Some(want) = &step.result {
                assert_eq!(result.as_ref(), Some(want), "{at}: result");
            }
            if let Some(want) = &step.ownership {
                assert_eq!(
                    &serde_json::to_value(o.report()).unwrap(),
                    want,
                    "{at}: ownership"
                );
            }
            for (name, want) in step.indicators.iter().flatten() {
                let shown = serde_json::to_value(o.indicator(group(name))).unwrap();
                assert_eq!(&shown, want, "{at}: indicator {name}");
            }
        }
    }
}
