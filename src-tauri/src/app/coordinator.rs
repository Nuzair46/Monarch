use super::state::MonarchManager;
use super::{
    commands::{snapshot_from_manager, AppSnapshotDto},
    events, shortcuts, startup,
};
use monarch::watchdog::{poll_confirmation, ConfirmationPoll, ConfirmationWatchdog};
use monarch::{AppSettings, Layout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc, RwLock,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Runtime};
use tokio::sync::oneshot;

pub enum Operation {
    Toggle(String, bool),
    ApplyLayout(Layout),
    ApplyProfile(String, bool),
    SaveProfile(String),
    DeleteProfile(String),
    Restore,
    Confirm,
    Rollback,
    Settings(AppSettings),
    ToggleCursor,
}

struct Request {
    operation: Operation,
    reply: oneshot::Sender<Result<(), String>>,
}
struct Published {
    snapshot: AppSnapshotDto,
    at: Instant,
}

#[derive(Clone)]
pub struct Controller {
    queue: SyncSender<Request>,
    published: Arc<RwLock<Published>>,
    refresh: Arc<AtomicBool>,
    invalidate: Arc<AtomicBool>,
}

impl Controller {
    pub fn start<R: Runtime>(app: AppHandle<R>, manager: MonarchManager) -> Result<Self, String> {
        let snapshot = snapshot_from_manager(&manager).map_err(|e| e.to_string())?;
        let (controller, receiver) = Self::channel(snapshot);
        let worker = controller.clone();
        std::thread::spawn(move || worker.run(app, manager, receiver));
        Ok(controller)
    }

    fn channel(snapshot: AppSnapshotDto) -> (Self, Receiver<Request>) {
        let (queue, receiver) = mpsc::sync_channel(16);
        let controller = Self {
            queue,
            published: Arc::new(RwLock::new(Published {
                snapshot,
                at: Instant::now(),
            })),
            refresh: Arc::new(AtomicBool::new(false)),
            invalidate: Arc::new(AtomicBool::new(false)),
        };
        (controller, receiver)
    }

    pub fn snapshot(&self) -> Result<AppSnapshotDto, String> {
        let published = self
            .published
            .read()
            .map_err(|_| "snapshot lock poisoned")?;
        let mut snapshot = published.snapshot.clone();
        if let Some(pending) = &mut snapshot.pending_confirmation {
            pending.remaining_ms = pending
                .remaining_ms
                .saturating_sub(published.at.elapsed().as_millis() as u64);
        }
        Ok(snapshot)
    }

    pub fn refresh(&self, invalidate: bool) {
        if invalidate {
            super::cursor::suspend();
            self.invalidate.store(true, Ordering::Release);
        }
        self.refresh.store(true, Ordering::Release);
    }

    pub fn submit(
        &self,
        operation: Operation,
    ) -> Result<oneshot::Receiver<Result<(), String>>, String> {
        let (reply, receiver) = oneshot::channel();
        self.queue
            .try_send(Request { operation, reply })
            .map_err(|e| match e {
                TrySendError::Full(_) => {
                    "Monarch is busy; the operation was not queued".to_string()
                }
                TrySendError::Disconnected(_) => "display worker stopped".to_string(),
            })?;
        Ok(receiver)
    }

    pub async fn execute(&self, operation: Operation) -> Result<(), String> {
        self.submit(operation)?
            .await
            .map_err(|_| "display worker stopped before reporting the result".to_string())?
    }

    fn publish(&self, manager: &MonarchManager) -> Result<(), String> {
        let observed = snapshot_from_manager(manager);
        let mut published = self
            .published
            .write()
            .map_err(|_| "snapshot lock poisoned")?;
        match observed {
            Ok(snapshot) => {
                *published = Published {
                    snapshot,
                    at: Instant::now(),
                };
                Ok(())
            }
            Err(error) => {
                // Enumeration failure must not hide a durable recovery transaction
                // or keep showing settings that have already been rolled back.
                super::commands::update_snapshot_metadata(&mut published.snapshot, manager);
                published.at = Instant::now();
                Err(error.to_string())
            }
        }
    }

    fn run<R: Runtime>(
        &self,
        app: AppHandle<R>,
        mut manager: MonarchManager,
        receiver: Receiver<Request>,
    ) {
        let mut last_refresh = Instant::now();
        let mut last_color: Option<Option<String>> = None;
        let mut recovery_id = None;
        let mut watchdog = ConfirmationWatchdog::new(Instant::now());
        loop {
            let pending = manager.pending_confirmation_started_at();
            if pending != recovery_id {
                recovery_id = pending;
                watchdog = ConfirmationWatchdog::new(Instant::now());
            }
            // Recovery gets the worker before any ordinary queued operation.
            if let Some(token) = pending {
                if manager
                    .pending_confirmation_remaining()
                    .is_some_and(|t| t.is_zero())
                {
                    super::cursor::suspend();
                }
                let result =
                    watchdog.poll(Instant::now(), || poll_confirmation(&mut manager, token));
                let event = match result {
                    Ok(ConfirmationPoll::Finished { reverted: true }) => {
                        Some(events::ConfirmationEvent::Reverted {
                            reason: events::ConfirmationRevertReason::Timeout,
                        })
                    }
                    Err(message) => Some(events::ConfirmationEvent::RollbackFailed { message }),
                    _ => None,
                };
                if let Some(event) = event {
                    let _ = self.publish(&manager);
                    events::emit_confirmation(&app, event);
                    events::refresh_tray_menu(&app);
                    events::emit_state_changed(&app);
                }
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(request) => {
                    let result = self.perform(&app, &mut manager, request.operation);
                    if let Err(error) = self.publish(&manager) {
                        crate::diagnostics::log(format!("snapshot:failed:{error}"));
                    }
                    let _ = request.reply.send(result);
                    let _ = shortcuts::sync_global_shortcuts(&app);
                    events::refresh_tray_menu(&app);
                    events::emit_state_changed(&app);
                    last_refresh = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if self.refresh.swap(false, Ordering::AcqRel)
                || last_refresh.elapsed() >= Duration::from_secs(2)
            {
                if self.invalidate.swap(false, Ordering::AcqRel) {
                    let _ = manager.invalidate_backend_cache();
                }
                if !manager.has_pending_confirmation() {
                    if let Ok(color) = manager.color_state_signature() {
                        let needs_calibration = last_color.as_ref().is_some_and(|old| {
                            events::should_auto_reapply_calibration(old.as_ref(), color.as_ref())
                        });
                        if !needs_calibration || manager.reapply_color_calibration().is_ok() {
                            last_color = Some(color);
                        }
                    }
                }
                if self.publish(&manager).is_ok() {
                    let _ = shortcuts::sync_global_shortcuts(&app);
                    events::refresh_tray_menu(&app);
                    events::emit_state_changed(&app);
                }
                last_refresh = Instant::now();
            }
        }
    }

    fn perform<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        manager: &mut MonarchManager,
        operation: Operation,
    ) -> Result<(), String> {
        if matches!(
            &operation,
            Operation::Toggle(..)
                | Operation::ApplyLayout(..)
                | Operation::ApplyProfile(..)
                | Operation::Restore
                | Operation::Rollback
        ) {
            super::cursor::suspend();
        }
        let mut auto_confirm = false;
        let before = manager.pending_confirmation_started_at();
        let result = match operation {
            Operation::Toggle(key, confirm) => {
                auto_confirm = confirm;
                manager.toggle_display(
                    &monarch::identity::parse_display_key(&key).map_err(|e| e.to_string())?,
                )
            }
            Operation::ApplyLayout(layout) => manager.apply_layout(layout),
            Operation::ApplyProfile(name, confirm) => {
                auto_confirm = confirm;
                manager.apply_profile(&name)
            }
            Operation::SaveProfile(name) => manager.save_profile(name),
            Operation::DeleteProfile(name) => manager.delete_profile(&name),
            Operation::Restore => {
                if manager.has_pending_confirmation() {
                    manager.rollback_pending()
                } else {
                    manager.restore_last_layout()
                }
            }
            Operation::Confirm => {
                let result = manager.confirm_current_layout();
                if result.is_ok() {
                    events::emit_confirmation(app, events::ConfirmationEvent::Confirmed);
                }
                result
            }
            Operation::Rollback => {
                let result = manager.rollback_pending();
                if result.is_ok() {
                    events::emit_confirmation(
                        app,
                        events::ConfirmationEvent::Reverted {
                            reason: events::ConfirmationRevertReason::Manual,
                        },
                    );
                }
                result
            }
            Operation::Settings(settings) => return self.settings(app, manager, settings),
            Operation::ToggleCursor => {
                let mut settings = manager.settings().clone();
                settings.cursor_correction_enabled = !settings.cursor_correction_enabled;
                return self.settings(app, manager, settings);
            }
        }
        .map_err(|e| e.to_string());
        if result.is_ok() && auto_confirm && manager.has_pending_confirmation() {
            manager
                .confirm_current_layout()
                .map_err(|e| e.to_string())?;
        }
        if manager.pending_confirmation_started_at() != before {
            if let Some(timeout) = manager.pending_confirmation_remaining() {
                events::emit_confirmation(
                    app,
                    events::ConfirmationEvent::Applied {
                        timeout_ms: timeout.as_millis() as u64,
                    },
                );
            }
        }
        result
    }

    fn settings<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        manager: &mut MonarchManager,
        next: AppSettings,
    ) -> Result<(), String> {
        let previous = manager.settings().clone();
        let startup_changed = previous.start_with_windows != next.start_with_windows;
        manager
            .update_settings(next.clone())
            .map_err(|e| e.to_string())?;
        let result = (|| {
            if startup_changed {
                startup::sync_start_with_windows(next.start_with_windows)?;
            }
            self.publish(manager)?;
            shortcuts::sync_global_shortcuts(app)
        })();
        if let Err(error) = result {
            let rollback = manager.update_settings(previous.clone());
            if startup_changed {
                let _ = startup::sync_start_with_windows(previous.start_with_windows);
            }
            let _ = self.publish(manager);
            let _ = shortcuts::sync_global_shortcuts(app);
            return Err(match rollback {
                Ok(()) => error,
                Err(e) => format!("{error}; settings recovery failed: {e}"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller() -> (Controller, Receiver<Request>) {
        Controller::channel(AppSnapshotDto {
            cursor_status: monarch::cursor::CursorStatus::default(),
            generation: 12,
            displays: Vec::new(),
            layout: super::super::commands::LayoutDto {
                outputs: Vec::new(),
            },
            profiles: Vec::new(),
            capabilities: Vec::new(),
            settings: AppSettings::default(),
            pending_confirmation: None,
        })
    }

    #[test]
    fn queue_saturation_is_rejected_without_blocking_snapshot_reads() {
        let (controller, requests) = controller();
        let mut replies = Vec::new();
        for _ in 0..16 {
            replies.push(controller.submit(Operation::Confirm).unwrap());
        }
        assert!(controller
            .submit(Operation::Restore)
            .unwrap_err()
            .contains("not queued"));
        assert_eq!(controller.snapshot().unwrap().generation, 12);
        // A blocked display worker does not own the published snapshot lock.
        let request = requests.recv().unwrap();
        assert_eq!(controller.snapshot().unwrap().generation, 12);
        assert!(controller.submit(Operation::Restore).is_ok());
        request.reply.send(Ok(())).unwrap();
        assert!(replies[0].try_recv().unwrap().is_ok());
    }

    #[test]
    fn disconnected_worker_reports_a_definitive_submission_failure() {
        let (controller, requests) = controller();
        drop(requests);
        assert!(controller
            .submit(Operation::Confirm)
            .unwrap_err()
            .contains("worker stopped"));
    }
}
