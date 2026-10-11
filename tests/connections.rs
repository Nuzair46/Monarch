use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use monarch::connections::ConnectionMonitor;
use monarch::*;

fn output(id: u32, enabled: bool) -> OutputConfig {
    OutputConfig {
        display_id: DisplayId {
            adapter_luid: 1,
            target_id: id,
            edid_hash: Some(id.into()),
            identity: Default::default(),
        },
        enabled,
        primary: id == 1,
        position: Position {
            x: (id as i32 - 1) * 1920,
            y: 0,
        },
        resolution: Resolution {
            width: 1920,
            height: 1080,
        },
        refresh_rate_mhz: 60_000,
        rotation: Some(Rotation::Landscape),
        hdr_enabled: Some(false),
        scale_percent: Some(100),
        clone_group: None,
    }
}

fn desk() -> Layout {
    Layout {
        outputs: vec![output(1, true), output(2, false), output(3, false)],
    }
}

fn connected() -> Layout {
    Layout {
        outputs: (1..=4).map(|id| output(id, true)).collect(),
    }
}

#[derive(Clone, Default)]
struct Store {
    inner: MemoryConfigStore,
    fail_after: Rc<Cell<Option<usize>>>,
}

impl ConfigStore for Store {
    fn load(&self) -> Result<AppConfig, ManagerError> {
        self.inner.load()
    }
    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        if let Some(remaining) = self.fail_after.get() {
            self.fail_after.set(remaining.checked_sub(1));
            if remaining == 0 {
                return Err(ManagerError::Backend("disk failure".into()));
            }
        }
        self.inner.save(config)
    }
}

#[derive(Clone)]
struct Backend {
    inner: MockBackend,
    applies: Rc<Cell<usize>>,
    failures: Rc<Cell<usize>>,
    ignore_next_apply: Rc<Cell<bool>>,
    change_on_read: Rc<RefCell<Option<(usize, Layout)>>>,
    audio_writes: Rc<Cell<usize>>,
}

impl DisplayBackend for Backend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.inner.list_displays()
    }
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        let mut change = self.change_on_read.borrow_mut();
        if let Some((remaining, layout)) = change.as_mut() {
            if *remaining == 0 {
                self.inner.apply_layout(layout.clone())?;
                *change = None;
            } else {
                *remaining -= 1;
            }
        }
        self.inner.get_layout()
    }
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        self.applies.set(self.applies.get() + 1);
        if self.ignore_next_apply.replace(false) {
            return Ok(());
        }
        self.inner.apply_layout(layout)?;
        if self.failures.get() > 0 {
            self.failures.set(self.failures.get() - 1);
            return Err(ManagerError::Backend("driver failure".into()));
        }
        Ok(())
    }
    fn audio_snapshot(&self) -> Result<AudioSnapshot, ManagerError> {
        Ok(AudioSnapshot::default())
    }
    fn set_audio_defaults(&self, _: &AudioDefaults) -> Result<(), ManagerError> {
        self.audio_writes.set(self.audio_writes.get() + 1);
        Ok(())
    }
}

struct Rig {
    backend: Backend,
    store: Store,
    manager: MonarchDisplayManager<Backend, Store>,
    monitor: ConnectionMonitor,
    start: Instant,
}

impl Rig {
    fn new(layout: Layout) -> Self {
        let backend = Backend {
            inner: MockBackend::new(vec![], layout).unwrap(),
            applies: Default::default(),
            failures: Default::default(),
            ignore_next_apply: Default::default(),
            change_on_read: Default::default(),
            audio_writes: Default::default(),
        };
        let store = Store::default();
        let manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
        let mut rig = Self {
            backend,
            store,
            manager,
            monitor: Default::default(),
            start: Instant::now(),
        };
        assert!(!rig.poll(0).unwrap());
        rig
    }
    fn poll(&mut self, millis: u64) -> Result<bool, ManagerError> {
        self.monitor.poll(
            &mut self.manager,
            self.start + Duration::from_millis(millis),
        )
    }
    fn external(&self, layout: Layout) {
        self.backend.inner.apply_layout(layout).unwrap();
    }
    fn live(&self) -> Layout {
        self.backend.inner.current_layout().unwrap()
    }
}

#[test]
fn vd_connection_keeps_new_display_and_current_settings_without_switching_audio() {
    let mut rig = Rig::new(desk());
    let mut vd = connected();
    vd.outputs[0].refresh_rate_mhz = 120_000;
    vd.outputs[3].hdr_enabled = Some(true);
    vd.outputs[3].scale_percent = Some(150);
    rig.external(vd.clone());
    for millis in [100, 350, 600, 850, 1099] {
        assert!(!rig.poll(millis).unwrap());
    }
    assert!(rig.poll(1100).unwrap());
    let mut expected = vd.clone();
    expected.outputs[1].enabled = false;
    expected.outputs[2].enabled = false;
    assert_eq!(rig.live(), expected);
    assert_eq!(rig.backend.applies.get(), 1);
    assert_eq!(rig.backend.audio_writes.get(), 0);
    assert!(!rig.manager.has_pending_confirmation());
    let saved = rig.store.load().unwrap();
    assert_eq!(saved.last_known_good_layout, Some(expected));
    assert_eq!(saved.last_restorable_layout, Some(vd));
    assert!(saved.pending_recovery.is_none());
    for millis in [1200, 2200, 4000] {
        assert!(!rig.poll(millis).unwrap());
    }
    assert_eq!(rig.backend.applies.get(), 1);
}

#[test]
fn stages_in_one_connection_burst_use_the_original_detached_state() {
    let mut rig = Rig::new(desk());
    let mut intermediate = desk();
    intermediate.outputs[1].enabled = true;
    rig.external(intermediate);
    assert!(!rig.poll(100).unwrap());
    rig.external(connected());
    assert!(!rig.poll(900).unwrap());
    assert!(!rig.poll(1100).unwrap());
    assert!(rig.poll(1900).unwrap());
    assert!(!rig.live().outputs[1].enabled);
    assert!(!rig.live().outputs[2].enabled);
}

#[test]
fn changing_burst_is_bounded_to_five_seconds() {
    let mut rig = Rig::new(desk());
    let mut vd = connected();
    rig.external(vd.clone());
    assert!(!rig.poll(100).unwrap());
    for millis in [900, 1700, 2500, 3300, 4100, 4900] {
        vd.outputs[3].position.x += 1;
        rig.external(vd.clone());
        assert!(!rig.poll(millis).unwrap());
    }
    assert!(rig.poll(5100).unwrap());
}

#[test]
fn normal_windows_edits_update_the_baseline_without_being_undone() {
    let mut rig = Rig::new(desk());
    let mut edited = desk();
    edited.outputs[1].enabled = true;
    rig.external(edited.clone());
    assert!(!rig.poll(100).unwrap());
    assert!(!rig.poll(1100).unwrap());
    assert_eq!(rig.live(), edited);
    rig.external(connected());
    assert!(!rig.poll(2000).unwrap());
    assert!(rig.poll(3000).unwrap());
    assert!(rig.live().outputs[1].enabled);
    assert!(!rig.live().outputs[2].enabled);
}

#[test]
fn disconnect_and_reconnect_preserve_detached_monitors() {
    let mut baseline = desk();
    baseline.outputs.push(output(4, true));
    let mut rig = Rig::new(baseline);
    let mut removed = connected();
    removed.outputs.pop();
    rig.external(removed);
    assert!(!rig.poll(100).unwrap());
    assert!(rig.poll(1100).unwrap());
    assert_eq!(rig.live(), desk());
    rig.external(connected());
    assert!(!rig.poll(2100).unwrap());
    assert!(rig.poll(3100).unwrap());
    assert!(rig.live().outputs[3].enabled);
}

#[test]
fn identity_remapping_and_inventory_order_are_not_connection_changes() {
    let mut rig = Rig::new(desk());
    let mut moved = connected();
    moved.outputs.pop();
    for o in &mut moved.outputs {
        o.display_id.target_id += 10;
    }
    moved.outputs.reverse();
    rig.external(moved.clone());
    assert!(!rig.poll(100).unwrap());
    assert!(!rig.poll(1100).unwrap());
    assert_eq!(rig.live(), moved);
    assert_eq!(rig.backend.applies.get(), 0);
}

#[test]
fn connection_correction_resolves_changed_display_ids() {
    let mut rig = Rig::new(desk());
    let mut vd = connected();
    for o in &mut vd.outputs {
        o.display_id.target_id += 10;
    }
    rig.external(vd);
    assert!(!rig.poll(100).unwrap());
    assert!(rig.poll(1100).unwrap());
    assert!(!rig.live().outputs[1].enabled);
    assert!(rig.live().outputs[3].enabled);
}

#[test]
fn ambiguous_or_reused_identities_never_disable_new_displays() {
    let mut baseline = desk();
    baseline.outputs.pop();
    let mut rig = Rig::new(baseline);
    let mut vd = connected();
    vd.outputs[1].display_id.edid_hash = Some(99); // reused endpoint, different monitor
    vd.outputs[2].display_id.edid_hash = Some(2);
    vd.outputs[3].display_id.edid_hash = Some(2); // ambiguous fallback
    rig.external(vd.clone());
    assert!(!rig.poll(100).unwrap());
    assert!(!rig.poll(1100).unwrap());
    assert_eq!(rig.live(), vd);
    assert_eq!(rig.backend.applies.get(), 0);
}

#[test]
fn unchanged_detached_outputs_need_no_apply() {
    let mut rig = Rig::new(desk());
    let mut vd = desk();
    vd.outputs.push(output(4, true));
    rig.external(vd);
    assert!(!rig.poll(100).unwrap());
    assert!(!rig.poll(1100).unwrap());
    assert_eq!(rig.backend.applies.get(), 0);
}

#[test]
fn correction_never_removes_the_last_active_display() {
    let mut rig = Rig::new(desk());
    let mut remaining = Layout {
        outputs: vec![output(2, true)],
    };
    remaining.outputs[0].primary = true;
    remaining.outputs[0].position.x = 0;
    rig.external(remaining.clone());
    assert!(!rig.poll(100).unwrap());
    assert!(rig.poll(1100).is_err());
    assert_eq!(rig.live(), remaining);
    assert!(!rig.poll(2100).unwrap());
    assert_eq!(rig.backend.applies.get(), 0);
}

#[test]
fn detaching_primary_clone_member_preserves_remaining_source() {
    let mut baseline = desk();
    baseline.outputs.pop();
    let mut rig = Rig::new(baseline);
    let mut vd = Layout {
        outputs: vec![output(1, true), output(2, true), output(4, true)],
    };
    vd.outputs[0].primary = false;
    vd.outputs[0].position.x = -1920;
    for index in [1, 2] {
        vd.outputs[index].primary = true;
        vd.outputs[index].position.x = 0;
        vd.outputs[index].clone_group = Some("pair".into());
    }
    rig.external(vd.clone());
    assert!(!rig.poll(100).unwrap());
    assert!(rig.poll(1100).unwrap());
    let actual = rig.live();
    assert!(!actual.outputs[1].enabled);
    assert!(!actual.outputs[1].primary);
    assert_eq!(actual.outputs[2].clone_group, None);
    assert!(actual.outputs[2].primary);
    vd.outputs[2].clone_group = None;
    assert_eq!(actual.outputs[2], vd.outputs[2]);
}

#[test]
fn pending_confirmation_cancels_correction_and_adopts_confirmed_state() {
    let mut rig = Rig::new(desk());
    rig.external(connected());
    assert!(!rig.poll(100).unwrap());
    rig.manager.apply_layout(connected()).unwrap();
    assert!(!rig.poll(1100).unwrap());
    rig.manager.confirm_current_layout().unwrap();
    assert!(!rig.poll(1200).unwrap());
    assert!(!rig.poll(2200).unwrap());
    assert_eq!(rig.live(), connected());
    assert_eq!(rig.backend.applies.get(), 1);
}

#[test]
fn explicit_action_reset_and_restart_adopt_live_state() {
    let mut rig = Rig::new(desk());
    rig.external(connected());
    assert!(!rig.poll(100).unwrap());
    rig.monitor.reset();
    assert!(!rig.poll(1100).unwrap());
    assert!(!rig.poll(2100).unwrap());
    rig.monitor = ConnectionMonitor::default();
    assert!(!rig.poll(3100).unwrap());
    assert!(!rig.poll(4100).unwrap());
    assert_eq!(rig.backend.applies.get(), 0);
}

#[test]
fn desktop_changes_during_preflight_cancel_the_correction() {
    let mut rig = Rig::new(desk());
    rig.external(connected());
    assert!(!rig.poll(100).unwrap());
    let mut newer = connected();
    newer.outputs.push(output(5, true));
    *rig.backend.change_on_read.borrow_mut() = Some((1, newer.clone()));
    assert!(!rig.poll(1100).unwrap());
    assert_eq!(rig.live(), newer);
    assert_eq!(rig.backend.applies.get(), 0);
    assert!(!rig.manager.has_pending_confirmation());
}

#[test]
fn apply_or_verification_failure_restores_pre_correction_layout_without_retrying() {
    for ignore_apply in [false, true] {
        let mut rig = Rig::new(desk());
        rig.external(connected());
        assert!(!rig.poll(100).unwrap());
        rig.backend.ignore_next_apply.set(ignore_apply);
        rig.backend.failures.set(usize::from(!ignore_apply));
        assert!(matches!(
            rig.poll(1100),
            Err(ManagerError::ApplyRestored(_))
        ));
        assert_eq!(rig.live(), connected());
        assert!(!rig.manager.has_pending_confirmation());
        assert!(rig.store.load().unwrap().pending_recovery.is_none());
        assert!(!rig.poll(2100).unwrap());
        assert!(!rig.poll(3100).unwrap());
        assert_eq!(rig.backend.applies.get(), 2);
    }
}

#[test]
fn failed_auto_confirmation_rolls_back_and_clears_the_journal() {
    let mut rig = Rig::new(desk());
    rig.external(connected());
    assert!(!rig.poll(100).unwrap());
    rig.store.fail_after.set(Some(1)); // journal succeeds; confirmation fails
    assert!(matches!(
        rig.poll(1100),
        Err(ManagerError::ApplyRestored(_))
    ));
    assert_eq!(rig.live(), connected());
    assert!(!rig.manager.has_pending_confirmation());
    assert!(rig.store.load().unwrap().pending_recovery.is_none());
}

#[test]
fn failed_rollback_keeps_recovery_due_and_suspends_connection_handling() {
    let mut rig = Rig::new(desk());
    rig.external(connected());
    assert!(!rig.poll(100).unwrap());
    rig.backend.failures.set(2);
    assert!(matches!(
        rig.poll(1100),
        Err(ManagerError::RecoveryRequired(_))
    ));
    assert_eq!(
        rig.store.load().unwrap().pending_recovery,
        Some(connected())
    );
    assert!(rig.manager.has_pending_confirmation());
    assert!(!rig.poll(2100).unwrap());
    assert_eq!(rig.backend.applies.get(), 2);
    assert!(rig.manager.rollback_if_confirmation_expired().unwrap());
    assert!(!rig.poll(3100).unwrap());
    assert!(!rig.poll(4100).unwrap());
    assert_eq!(rig.backend.applies.get(), 3);
}
