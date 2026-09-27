use monarch::{
    cursor::{InputContext, Mapping, Point, Rect},
    AppSettings, Layout,
};
use std::{
    cell::RefCell,
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};
use windows::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW, StationsAndDesktops::*, Threading::GetCurrentThreadId,
    },
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static SUSPENDED: AtomicBool = AtomicBool::new(true);
static EPOCH: AtomicU64 = AtomicU64::new(0);
static SERVICE: OnceLock<Mutex<Option<HookThread>>> = OnceLock::new();
const WAKE: u32 = WM_APP + 47;
const INJECTED_TAG: usize = 0x4d4f4e41;
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}
pub fn suspend() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
    SUSPENDED.store(true, Ordering::Release);
}
enum Command {
    Replace(Mapping, u64),
    Stop,
}
struct HookThread {
    sender: mpsc::Sender<Command>,
    id: u32,
    join: Option<thread::JoinHandle<()>>,
    mapping: Mapping,
    epoch: u64,
}
impl Drop for HookThread {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Stop);
        unsafe {
            let _ = PostThreadMessageW(self.id, WAKE, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Release);
    suspend();
    if let Some(service) = SERVICE.get() {
        if let Ok(mut service) = service.lock() {
            service.take();
        }
    }
}
pub fn sync(settings: &AppSettings, layout: &Layout, observed_epoch: u64) -> Result<(), String> {
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return Ok(());
    }
    let mapping = Mapping::build(layout, &settings.cursor_calibrations);
    let mut service = SERVICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "cursor service lock poisoned")?;
    if !settings.cursor_correction_enabled || mapping.surfaces.len() < 2 {
        SUSPENDED.store(true, Ordering::Release);
        service.take();
        return Ok(());
    }
    if service
        .as_ref()
        .is_some_and(|running| running.join.as_ref().is_some_and(|join| join.is_finished()))
    {
        service.take();
    }
    if let Some(running) = service.as_mut() {
        if running.mapping == mapping && running.epoch == observed_epoch {
            return Ok(());
        }
        running
            .sender
            .send(Command::Replace(mapping.clone(), observed_epoch))
            .map_err(|_| "cursor thread stopped")?;
        unsafe {
            PostThreadMessageW(running.id, WAKE, WPARAM(0), LPARAM(0))
                .map_err(|e| format!("cursor wake failed: {e}"))?;
        }
        running.mapping = mapping;
        running.epoch = observed_epoch;
    } else {
        let (sender, receiver) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("monarch-cursor".into())
            .spawn(move || run(receiver, ready))
            .map_err(|e| e.to_string())?;
        let id = started
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "cursor hook startup timed out")??;
        sender
            .send(Command::Replace(mapping.clone(), observed_epoch))
            .map_err(|_| "cursor thread stopped")?;
        unsafe {
            PostThreadMessageW(id, WAKE, WPARAM(0), LPARAM(0)).map_err(|e| e.to_string())?;
        }
        *service = Some(HookThread {
            sender,
            id,
            join: Some(join),
            mapping,
            epoch: observed_epoch,
        });
    }
    Ok(())
}
#[derive(Default)]
struct State {
    mapping: Mapping,
    previous: Option<Point>,
    desktop_available: bool,
    epoch: u64,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
struct Hook(HHOOK);
impl Drop for Hook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}
fn run(receiver: mpsc::Receiver<Command>, ready: mpsc::SyncSender<Result<u32, String>>) {
    unsafe {
        windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        // Force creation of this thread's message queue before publishing its ID.
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        let module = match GetModuleHandleW(None) {
            Ok(m) => m,
            Err(e) => {
                let _ = ready.send(Err(e.to_string()));
                return;
            }
        };
        let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(module.into()), 0) {
            Ok(h) => Hook(h),
            Err(e) => {
                let _ = ready.send(Err(format!("Could not install cursor hook: {e}")));
                return;
            }
        };
        let timer = SetTimer(None, 0, 250, None);
        if timer == 0 {
            let _ = ready.send(Err("Could not start input-desktop checks".into()));
            return;
        }
        if ready.send(Ok(GetCurrentThreadId())).is_err() {
            let _ = KillTimer(None, timer);
            return;
        }
        loop {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status <= 0 {
                break;
            }
            let mut stop = false;
            loop {
                match receiver.try_recv() {
                    Ok(Command::Replace(mapping, observed_epoch)) => {
                        STATE.with(|state| {
                            *state.borrow_mut() = State {
                                mapping,
                                previous: None,
                                desktop_available: input_desktop_available(),
                                epoch: observed_epoch,
                            }
                        });
                        if observed_epoch == epoch() {
                            SUSPENDED.store(false, Ordering::Release);
                        }
                    }
                    Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                        stop = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if stop {
                break;
            }
            if message.message == WM_TIMER {
                let available = input_desktop_available();
                STATE.with(|state| {
                    let mut s = state.borrow_mut();
                    if s.desktop_available != available {
                        s.previous = None;
                    }
                    s.desktop_available = available;
                });
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        let _ = KillTimer(None, timer);
        drop(hook);
    }
}
unsafe fn desktop_name(desktop: HDESK) -> Option<Vec<u16>> {
    let mut name = [0u16; 256];
    GetUserObjectInformationW(
        HANDLE(desktop.0),
        UOI_NAME,
        Some(name.as_mut_ptr().cast()),
        size_of::<[u16; 256]>() as u32,
        None,
    )
    .ok()?;
    Some(name[..name.iter().position(|n| *n == 0)?].to_vec())
}
unsafe fn input_desktop_available() -> bool {
    let Ok(input) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else {
        return false;
    };
    let input_name = desktop_name(input);
    let _ = CloseDesktop(input);
    let current = GetThreadDesktop(GetCurrentThreadId())
        .ok()
        .and_then(|d| desktop_name(d));
    input_name.is_some() && input_name == current
}
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 || wparam.0 != WM_MOUSEMOVE as usize {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let input = &*(lparam.0 as *const MSLLHOOKSTRUCT);
    if input.dwExtraInfo == INJECTED_TAG {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let corrected = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return false;
        };
        let proposed = Point {
            x: input.pt.x,
            y: input.pt.y,
        };
        if SUSPENDED.load(Ordering::Acquire)
            || state.epoch != epoch()
            || input.flags & LLMHF_INJECTED != 0
        {
            state.previous = None;
            return false;
        }
        let mut clip = RECT::default();
        let bounds = state.mapping.desktop;
        let confined = GetClipCursor(&mut clip).is_err()
            || Rect {
                left: clip.left,
                top: clip.top,
                right: clip.right,
                bottom: clip.bottom,
            } != bounds;
        let context = InputContext {
            injected: false,
            control_down: GetAsyncKeyState(i32::from(VK_CONTROL.0)) < 0,
            confined,
            input_desktop_available: state.desktop_available,
        };
        let previous = state.previous.replace(proposed);
        let Some(mapped) =
            previous.and_then(|previous| state.mapping.map_motion(previous, proposed, context))
        else {
            return false;
        };
        let width = i64::from(bounds.right - bounds.left);
        let height = i64::from(bounds.bottom - bounds.top);
        if width <= 0 || height <= 0 {
            return false;
        }
        let movement = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: ((i64::from(mapped.x - bounds.left) * 65536 + 32768) / width)
                        .clamp(0, 65535) as i32,
                    dy: ((i64::from(mapped.y - bounds.top) * 65536 + 32768) / height)
                        .clamp(0, 65535) as i32,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    dwExtraInfo: INJECTED_TAG,
                    ..Default::default()
                },
            },
        };
        if SendInput(&[movement], size_of::<INPUT>() as i32) != 1 {
            return false;
        }
        state.previous = Some(mapped);
        true
    });
    if corrected {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wparam, lparam)
    }
}
