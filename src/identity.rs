//! One matching policy for profiles, toggles, cached geometry and recovery.
use crate::{DisplayId, Layout};
use std::collections::HashSet;

#[derive(Debug, PartialEq)]
pub enum Resolution {
    Resolved(DisplayId),
    Missing,
    Ambiguous(Vec<DisplayId>),
}

fn unique(candidates: Vec<DisplayId>) -> Resolution {
    match candidates.as_slice() {
        [] => Resolution::Missing,
        [id] => Resolution::Resolved(id.clone()),
        _ => Resolution::Ambiguous(candidates),
    }
}

pub fn resolve(requested: &DisplayId, current: &Layout) -> Resolution {
    let ids: Vec<_> = current.outputs.iter().map(|o| &o.display_id).collect();
    let matching = |predicate: &dyn Fn(&DisplayId) -> bool| {
        ids.iter()
            .filter(|id| predicate(id))
            .map(|id| (*id).clone())
            .collect::<Vec<_>>()
    };
    let compatible = |id: &DisplayId| {
        requested
            .identity
            .edid_serial
            .as_ref()
            .zip(id.identity.edid_serial.as_ref())
            .is_none_or(|(a, b)| a == b)
    };
    let serial_matches = |id: &DisplayId| {
        requested
            .identity
            .edid_serial
            .as_ref()
            .is_some_and(|serial| id.identity.edid_serial.as_ref() == Some(serial))
    };
    let path_matches = |id: &DisplayId| {
        requested
            .identity
            .device_path
            .as_ref()
            .is_some_and(|path| id.identity.device_path.as_ref() == Some(path))
    };
    let hash_matches =
        |id: &DisplayId| requested.edid_hash.is_some() && requested.edid_hash == id.edid_hash;
    let legacy = requested.edid_hash.is_none()
        && requested.identity.edid_serial.is_none()
        && requested.identity.device_path.is_none();
    // Endpoints can be reused after reconnect/reboot. Require agreeing evidence
    // whenever the saved record has it, and never override a conflicting serial.
    let exact = matching(&|id| {
        id.endpoint() == requested.endpoint()
            && compatible(id)
            && (serial_matches(id) || path_matches(id) || hash_matches(id) || legacy)
    });
    if !exact.is_empty() {
        return unique(exact);
    }
    let serial = matching(&serial_matches);
    if !serial.is_empty() {
        return unique(serial);
    }
    let paths = matching(&|id| compatible(id) && path_matches(id));
    if !paths.is_empty() {
        return unique(paths);
    }
    if requested.edid_hash.is_some() {
        return unique(matching(&|id| compatible(id) && hash_matches(id)));
    }
    // Only hashless legacy records may fall back to a target number. A failed
    // identity query must not turn a modern saved identity into a weak match.
    if legacy {
        unique(matching(&|id| id.target_id == requested.target_id))
    } else {
        Resolution::Missing
    }
}

pub fn remap_layout(desired: &Layout, current: &Layout) -> Layout {
    let mut result = desired.clone();
    let mut used = HashSet::new();
    for output in &mut result.outputs {
        if let Resolution::Resolved(id) = resolve(&output.display_id, current) {
            if used.insert(id.endpoint()) {
                output.display_id = id;
            }
        }
    }
    result
}

/// Resolve every enabled target before building native routes. An unresolved ID
/// must not become usable merely because Windows reused its endpoint.
pub fn resolve_layout(desired: &Layout, current: &Layout) -> Result<Layout, crate::ManagerError> {
    let result = remap_layout(desired, current);
    for output in result.outputs.iter().filter(|o| o.enabled) {
        if !current
            .outputs
            .iter()
            .any(|o| o.display_id == output.display_id)
        {
            let reason = match resolve(&output.display_id, current) {
                Resolution::Ambiguous(_) => "ambiguous",
                _ => "missing or changed",
            };
            return Err(crate::ManagerError::Validation(format!(
                "display identity is {reason}; refresh the inventory before applying"
            )));
        }
    }
    result.ensure_valid()?;
    Ok(result)
}

/// After a resolved association, retain evidence absent from a transient query.
/// New evidence wins; callers must resolve the association before using this.
pub fn preserve_evidence(previous: &DisplayId, current: &mut DisplayId) {
    if current.identity.edid_serial.is_none() {
        current.identity.edid_serial = previous.identity.edid_serial.clone();
    }
    if current.identity.device_path.is_none() {
        current.identity.device_path = previous.identity.device_path.clone();
    }
}

pub fn display_key(id: &DisplayId) -> String {
    let hash = id
        .edid_hash
        .map(|v| format!("{v:016x}"))
        .unwrap_or_else(|| "-".into());
    format!("{:016x}:{}:{hash}", id.adapter_luid, id.target_id)
}

pub fn parse_display_key(key: &str) -> Result<DisplayId, crate::ManagerError> {
    let parts: Vec<_> = key.split(':').collect();
    let invalid = || crate::ManagerError::Validation("invalid display key".into());
    if !(2..=3).contains(&parts.len()) {
        return Err(invalid());
    }
    Ok(DisplayId {
        adapter_luid: u64::from_str_radix(parts[0], 16).map_err(|_| invalid())?,
        target_id: parts[1].parse().map_err(|_| invalid())?,
        edid_hash: match parts.get(2) {
            None | Some(&"-") => None,
            Some(value) => Some(u64::from_str_radix(value, 16).map_err(|_| invalid())?),
        },
        identity: Default::default(),
    })
}

/// EDID base-block serial, qualified by manufacturer/product. Invalid or missing
/// serials stay unknown; callers must also handle duplicate serials as ambiguous.
pub fn edid_serial(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 128
        || bytes[..8] != [0, 255, 255, 255, 255, 255, 255, 0]
        || bytes[..128].iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) != 0
    {
        return None;
    }
    let numeric = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    let text = bytes[54..126]
        .chunks_exact(18)
        .find(|d| d[..4] == [0, 0, 0, 255])
        .and_then(|d| std::str::from_utf8(&d[5..18]).ok())
        .map(|s| {
            s.trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_ascii_uppercase()
        })
        .filter(|s| {
            !s.is_empty()
                && s.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                && s.chars().any(|c| c != '0')
        });
    let serial =
        text.or_else(|| (numeric != 0 && numeric != u32::MAX).then(|| numeric.to_string()))?;
    Some(format!(
        "{:02x}{:02x}:{:02x}{:02x}:{serial}",
        bytes[8], bytes[9], bytes[10], bytes[11]
    ))
}
