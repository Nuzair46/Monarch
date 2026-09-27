use monarch::identity::{self, Resolution as Match};
use monarch::*;

fn id(adapter: u64, target: u32, serial: Option<&str>, path: Option<&str>) -> DisplayId {
    DisplayId {
        adapter_luid: adapter,
        target_id: target,
        edid_hash: Some(17),
        identity: MonitorIdentity {
            edid_serial: serial.map(str::to_owned),
            device_path: path.map(str::to_owned),
        },
    }
}
fn inventory(ids: Vec<DisplayId>) -> Layout {
    Layout {
        outputs: ids
            .into_iter()
            .map(|display_id| OutputConfig {
                display_id,
                enabled: false,
                primary: false,
                rotation: None,
                hdr_enabled: None,
                scale_percent: None,
                clone_group: None,
                position: Position { x: 0, y: 0 },
                resolution: Resolution {
                    width: 0,
                    height: 0,
                },
                refresh_rate_mhz: 60_000,
            })
            .collect(),
    }
}

#[test]
fn serial_survives_dock_port_and_adapter_changes() {
    let saved = id(1, 1, Some("ACME:panel123"), Some("port-a"));
    let mut moved = id(99, 7, Some("ACME:panel123"), Some("port-b"));
    moved.edid_hash = Some(999);
    assert_eq!(
        identity::resolve(&saved, &inventory(vec![moved.clone()])),
        Match::Resolved(moved)
    );
}

#[test]
fn conflicting_serial_never_falls_back_to_reused_endpoint_path_or_hash() {
    let saved = id(1, 1, Some("panel123"), Some("port-a"));
    let replacement = id(1, 1, Some("panel456"), Some("port-a"));
    assert_eq!(
        identity::resolve(&saved, &inventory(vec![replacement])),
        Match::Missing
    );
}

#[test]
fn duplicate_serials_after_a_move_are_ambiguous() {
    let saved = id(1, 1, Some("duplicated"), None);
    let twins = vec![
        id(2, 2, Some("duplicated"), None),
        id(2, 3, Some("duplicated"), None),
    ];
    assert_eq!(
        identity::resolve(&saved, &inventory(twins.clone())),
        Match::Ambiguous(twins)
    );
}

#[test]
fn saved_connection_distinguishes_identical_panels() {
    let saved = id(1, 1, Some("duplicated"), Some("port-a"));
    let twin = id(1, 2, Some("duplicated"), Some("port-b"));
    assert_eq!(
        identity::resolve(&saved, &inventory(vec![saved.clone(), twin])),
        Match::Resolved(saved)
    );
}

#[test]
fn known_identity_does_not_degrade_to_target_number_after_query_failure() {
    let mut saved = id(1, 1, Some("panel123"), Some("port-a"));
    saved.edid_hash = None;
    let mut unknown = id(1, 1, None, None);
    unknown.edid_hash = None;
    assert_eq!(
        identity::resolve(&saved, &inventory(vec![unknown])),
        Match::Missing
    );
}

#[test]
fn display_keys_round_trip_and_reject_trailing_data() {
    let saved = id(11, 42, None, None);
    assert_eq!(
        identity::parse_display_key(&identity::display_key(&saved)).unwrap(),
        saved
    );
    for invalid in ["", "a:2", "a:2:3:4", "no:2:3", "a:-2:3", "a:2:z"] {
        assert!(identity::parse_display_key(invalid).is_err());
    }
}

#[test]
fn target_number_alone_does_not_identify_a_monitor_after_an_adapter_change() {
    let mut saved = id(1, 1, None, None);
    saved.edid_hash = None;
    let connected = id(2, 1, None, None);
    assert_eq!(
        identity::resolve(&saved, &inventory(vec![connected])),
        Match::Missing
    );
}

fn edid(serial: u32) -> [u8; 128] {
    let mut bytes = [0; 128];
    bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
    bytes[8..12].copy_from_slice(&[4, 67, 8, 9]);
    bytes[12..16].copy_from_slice(&serial.to_le_bytes());
    checksum(&mut bytes);
    bytes
}
fn checksum(bytes: &mut [u8; 128]) {
    bytes[127] = 0u8.wrapping_sub(bytes[..127].iter().fold(0u8, |sum, b| sum.wrapping_add(*b)));
}

#[test]
fn edid_requires_valid_header_checksum_and_nonempty_serial() {
    assert_eq!(
        identity::edid_serial(&edid(123)).as_deref(),
        Some("0443:0809:123")
    );
    assert_eq!(identity::edid_serial(&edid(0)), None);
    assert_eq!(identity::edid_serial(&edid(u32::MAX)), None);
    let mut invalid = edid(123);
    invalid[40] = 1;
    assert_eq!(identity::edid_serial(&invalid), None);
    invalid[0] = 1;
    checksum(&mut invalid);
    assert_eq!(identity::edid_serial(&invalid), None);
    assert_eq!(identity::edid_serial(&[0; 127]), None);
}

#[test]
fn text_serial_is_preferred_and_control_characters_are_rejected() {
    let mut bytes = edid(123);
    bytes[54..59].copy_from_slice(&[0, 0, 0, 255, 0]);
    bytes[59..72].copy_from_slice(b"serial42\n    ");
    checksum(&mut bytes);
    assert_eq!(
        identity::edid_serial(&bytes).as_deref(),
        Some("0443:0809:SERIAL42")
    );
    bytes[61] = 1;
    checksum(&mut bytes);
    assert_eq!(
        identity::edid_serial(&bytes).as_deref(),
        Some("0443:0809:123")
    );
}

#[test]
fn fresh_native_resolution_rejects_replacement_at_a_previously_valid_endpoint() {
    let mut desired = inventory(vec![id(1, 1, Some("saved-panel"), Some("port"))]);
    desired.outputs[0].enabled = true;
    let current = inventory(vec![id(1, 1, Some("replacement"), Some("port"))]);
    assert!(identity::resolve_layout(&desired, &current).is_err());
}
