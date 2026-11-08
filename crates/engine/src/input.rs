//! Input decisions run on backend control threads, never audio callbacks.

use crate::DeviceInfo;

#[derive(Clone, Debug)]
pub struct Source {
    pub device: DeviceInfo,
    pub priority: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub device: DeviceInfo,
    pub fallback: bool,
}

/// `sources` contains only eligible microphones, excluding monitors and our devices.
pub fn choose(
    default: Option<&str>,
    sources: &[Source],
    previous: Option<&str>,
    saved: Option<&str>,
) -> Option<Selection> {
    let find = |id: Option<&str>| sources.iter().find(|s| Some(s.device.id.as_str()) == id);
    let primary = find(default);
    let source = primary
        .or_else(|| find(previous))
        .or_else(|| find(saved))
        .or_else(|| {
            sources.iter().max_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| b.device.id.cmp(&a.device.id))
            })
        })?;
    Some(Selection {
        device: source.device.clone(),
        fallback: primary.is_none(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<Source> {
        [("a", 1), ("b", 10), ("c", 10)]
            .into_iter()
            .map(|(id, priority)| Source {
                device: DeviceInfo {
                    id: id.into(),
                    label: id.into(),
                },
                priority,
            })
            .collect()
    }

    #[test]
    fn keeps_capture_for_self_default_and_follows_a_real_default() {
        let sources = sources();
        let kept = choose(Some("voice-strip"), &sources, Some("a"), Some("b")).unwrap();
        assert_eq!(kept.device.id, "a");
        assert!(kept.fallback);
        let real = choose(Some("c"), &sources, Some("a"), Some("b")).unwrap();
        assert_eq!(real.device.id, "c");
        assert!(!real.fallback);
    }

    #[test]
    fn saved_then_priority_then_stable_order_when_previous_disappears() {
        let mut sources = sources();
        assert_eq!(
            choose(Some("self"), &sources, Some("gone"), Some("a"))
                .unwrap()
                .device
                .id,
            "a"
        );
        assert_eq!(
            choose(Some("self"), &sources, None, Some("gone"))
                .unwrap()
                .device
                .id,
            "b"
        );
        sources.reverse();
        assert_eq!(choose(None, &sources, None, None).unwrap().device.id, "b");
        assert!(choose(Some("self"), &[], None, None).is_none());
    }
}
