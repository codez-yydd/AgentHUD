//! Read the selected Clash Verge profile's cached subscription counters.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use yaml_serde::Value;

const MAX_PROFILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Subscription {
    pub(super) used: u64,
    pub(super) total: u64,
}

#[derive(Deserialize)]
struct Profiles {
    current: Option<String>,
    items: Option<Vec<Profile>>,
}

#[derive(Deserialize)]
struct Profile {
    uid: Option<String>,
    extra: Option<Counters>,
}

#[derive(Deserialize)]
struct Counters {
    upload: Option<Value>,
    download: Option<Value>,
    total: Option<Value>,
}

fn bytes(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn parse(input: &str) -> Option<Subscription> {
    let profiles: Profiles = yaml_serde::from_str(input).ok()?;
    let current = profiles.current.filter(|uid| !uid.trim().is_empty())?;
    let profile = profiles
        .items?
        .into_iter()
        .find(|item| item.uid.as_deref() == Some(current.as_str()))?;
    let Counters {
        upload,
        download,
        total,
    } = profile.extra?;
    let total = bytes(&total?).filter(|total| *total > 0)?;
    let used = bytes(&upload?)?.checked_add(bytes(&download?)?)?;
    Some(Subscription { used, total })
}

pub(super) fn load(dir: &Path) -> Option<Subscription> {
    let path = dir.join("profiles.yaml");
    if !path.is_file() {
        return None;
    }
    let mut input = String::new();
    File::open(path)
        .ok()?
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_string(&mut input)
        .ok()?;
    if input.len() as u64 > MAX_PROFILE_BYTES {
        return None;
    }
    parse(&input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_selected_profile_even_when_another_subscription_precedes_it() {
        let yaml = r#"
current: selected
items:
- uid: other
  extra: {upload: 12, total: 100}
- uid: selected
  url: https://example.invalid/private
  selected:
  - name: GROUP
    now: DIRECT
  extra:
    upload: 9
    download: '16'
    total: 20
"#;
        assert_eq!(
            parse(yaml),
            Some(Subscription {
                used: 25,
                total: 20,
            })
        );
    }

    #[test]
    fn missing_or_unlimited_selected_quota_does_not_use_another_profile() {
        let yaml = r#"
current: selected
items:
- uid: selected
  extra: {upload: 0, download: 0, total: 0}
- uid: other
  extra: {upload: 1, download: 1, total: 10}
"#;
        assert_eq!(parse(yaml), None);
        assert_eq!(
            parse(&yaml.replace("current: selected", "current: missing")),
            None
        );
    }

    #[test]
    fn rejects_invalid_counters_and_addition_overflow() {
        for counters in [
            "upload: -1, download: 0, total: 1",
            "upload: 18446744073709551615, download: 1, total: 50",
            "upload: 0, total: 12",
        ] {
            assert_eq!(
                parse(&format!(
                    "current: selected\nitems:\n- uid: selected\n  extra: {{{counters}}}"
                )),
                None
            );
        }
    }
}
