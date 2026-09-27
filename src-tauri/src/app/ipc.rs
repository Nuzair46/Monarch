use tauri::{AppHandle, Runtime};

#[path = "ipc_protocol.rs"]
mod protocol;
use protocol::IpcRequest;

pub fn send_apply_profile_request(name: &str) -> Result<(), String> {
    send(IpcRequest::ApplyProfile { name: name.into() })
}
pub fn send_show_main_window_request() -> Result<(), String> {
    send(IpcRequest::ShowMainWindow)
}

#[cfg(not(windows))]
fn send(_request: IpcRequest) -> Result<(), String> {
    Err("single-instance IPC is supported on Windows".into())
}
#[cfg(not(windows))]
pub fn spawn_listener<R: Runtime>(_app: AppHandle<R>) {}
#[cfg(windows)]
fn send(request: IpcRequest) -> Result<(), String> {
    tauri::async_runtime::block_on(native::send(request))
}
#[cfg(windows)]
pub fn spawn_listener<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = native::serve(app).await {
            crate::diagnostics::log(format!("ipc:failed:{error}"));
        }
    });
}

#[cfg(windows)]
mod native {
    use super::protocol::{
        await_response, read_frame, write_frame, IpcResponse, COMPLETION_TIMEOUT, IO_TIMEOUT,
    };
    use super::*;
    use crate::app::{coordinator::Operation, state::MonarchAppState};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    use std::time::Duration;
    use tauri::Manager;
    use tokio::io::BufReader;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use tokio::sync::Semaphore;
    use tokio::time::timeout;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    fn address() -> Result<(String, String), String> {
        let (sid, session) = crate::app::session::user_session()?;
        Ok((format!(r"\\.\pipe\Monarch-{sid}-{session}"), sid))
    }

    fn create_pipe(first: bool) -> Result<NamedPipeServer, String> {
        let (address, sid) = address()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(|e| e.to_string())?;
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let pipe = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .max_instances(16)
                .create_with_security_attributes_raw(
                    address,
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                );
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            pipe.map_err(|e| e.to_string())
        }
    }

    pub async fn send(request: IpcRequest) -> Result<(), String> {
        let (address, _) = address()?;
        let mut pipe = timeout(IO_TIMEOUT, async {
            loop {
                match ClientOptions::new().open(&address) {
                    Ok(pipe) => return Ok(pipe),
                    Err(e) if matches!(e.raw_os_error(), Some(2 | 231)) => {
                        tokio::time::sleep(Duration::from_millis(50)).await
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
        })
        .await
        .map_err(|_| "running instance did not become ready".to_string())??;
        write_frame(&mut pipe, &request).await?;
        let mut reader = BufReader::new(pipe);
        await_response(&mut reader, IO_TIMEOUT, COMPLETION_TIMEOUT).await
    }

    pub async fn serve<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
        let readers = Arc::new(Semaphore::new(8));
        let profiles = Arc::new(Semaphore::new(2));
        let ids = AtomicU64::new(1);
        let mut listener = create_pipe(true)?;
        loop {
            listener.connect().await.map_err(|e| e.to_string())?;
            let next = create_pipe(false)?;
            let pipe = std::mem::replace(&mut listener, next);
            let Ok(reader_permit) = readers.clone().try_acquire_owned() else {
                continue;
            };
            let profiles = profiles.clone();
            let app = app.clone();
            let request_id = ids.fetch_add(1, Ordering::Relaxed);
            tauri::async_runtime::spawn(async move {
                let mut pipe = BufReader::new(pipe);
                let request: IpcRequest = match timeout(IO_TIMEOUT, read_frame(&mut pipe)).await {
                    Ok(Ok(request)) => request,
                    _ => return,
                };
                let (result, _profile_permit) = match request {
                    IpcRequest::ShowMainWindow => {
                        crate::app::events::show_main_window(&app);
                        let _ = write_frame(
                            &mut pipe,
                            &IpcResponse {
                                request_id,
                                completed: true,
                                error: None,
                            },
                        )
                        .await;
                        return;
                    }
                    IpcRequest::ApplyProfile { name } => {
                        let permit = match profiles.try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                let _ = write_frame(
                                    &mut pipe,
                                    &IpcResponse {
                                        request_id,
                                        completed: true,
                                        error: Some(
                                            "Monarch is busy; request was not queued".into(),
                                        ),
                                    },
                                )
                                .await;
                                return;
                            }
                        };
                        let result = app
                            .state::<MonarchAppState>()
                            .controller
                            .submit(Operation::ApplyProfile(name, true));
                        (result, permit)
                    }
                };
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = write_frame(
                            &mut pipe,
                            &IpcResponse {
                                request_id,
                                completed: true,
                                error: Some(error),
                            },
                        )
                        .await;
                        return;
                    }
                };
                drop(reader_permit);
                let _ = write_frame(
                    &mut pipe,
                    &IpcResponse {
                        request_id,
                        completed: false,
                        error: None,
                    },
                )
                .await;
                let error = match result.await {
                    Ok(result) => result.err(),
                    Err(_) => Some("worker stopped; outcome unknown".into()),
                };
                let _ = write_frame(
                    &mut pipe,
                    &IpcResponse {
                        request_id,
                        completed: true,
                        error,
                    },
                )
                .await;
            });
        }
    }
}
