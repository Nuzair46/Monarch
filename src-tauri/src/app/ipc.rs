use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

use crate::app::events;

const IPC_BIND_ADDR: &str = "127.0.0.1:42197";
const IPC_IO_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CONCURRENT_IPC_READERS: usize = 8;
const MAX_CONCURRENT_PROFILE_REQUESTS: usize = 2;
const MAX_REQUEST_BYTES: u64 = 8192;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcRequest {
    ApplyProfile {
        name: String,
    },
    #[serde(alias = "show_window")]
    ShowMainWindow,
}

#[derive(Serialize, Deserialize)]
struct IpcResponse {
    ok: bool,
    error: Option<String>,
}

pub fn send_apply_profile_request(profile_name: &str) -> Result<(), String> {
    send_request(IpcRequest::ApplyProfile {
        name: profile_name.to_string(),
    })
}

pub fn send_show_main_window_request() -> Result<(), String> {
    send_request(IpcRequest::ShowMainWindow)
}

fn send_request(request: IpcRequest) -> Result<(), String> {
    let mut stream = TcpStream::connect(IPC_BIND_ADDR)
        .map_err(|err| format!("failed to connect to running Monarch instance: {err}"))?;
    stream
        .set_read_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|err| format!("failed to set IPC read timeout: {err}"))?;
    stream
        .set_write_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|err| format!("failed to set IPC write timeout: {err}"))?;

    let mut request_bytes = serde_json::to_vec(&request)
        .map_err(|err| format!("failed to encode IPC request: {err}"))?;
    request_bytes.push(b'\n');
    stream
        .write_all(&request_bytes)
        .map_err(|err| format!("failed to send IPC request: {err}"))?;

    let mut response_line = String::new();
    let mut reader = BufReader::new(stream);
    reader
        .read_line(&mut response_line)
        .map_err(|err| format!("failed to read IPC response: {err}"))?;

    let response_line = response_line.trim();
    if response_line.is_empty() {
        return Err("received empty IPC response".to_string());
    }

    let response: IpcResponse = serde_json::from_str(response_line)
        .map_err(|err| format!("failed to decode IPC response: {err}"))?;
    if response.ok {
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "running instance rejected IPC command".to_string()))
    }
}

pub fn spawn_listener<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(IPC_BIND_ADDR) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("Monarch IPC listener bind failed on {IPC_BIND_ADDR}: {err}");
                return;
            }
        };

        serve_connections(listener.incoming(), move |request| match request {
            IpcRequest::ApplyProfile { name } => {
                events::apply_profile_external_action_result(&app, &name)
            }
            IpcRequest::ShowMainWindow => {
                events::show_main_window(&app);
                Ok(())
            }
        });
    });
}

struct Permit(Arc<AtomicUsize>);

impl Permit {
    fn acquire(counter: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < limit).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(counter.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve_connections<I, F>(connections: I, handler: F)
where
    I: IntoIterator<Item = std::io::Result<TcpStream>>,
    F: Fn(IpcRequest) -> Result<(), String> + Send + Sync + 'static,
{
    let readers = Arc::new(AtomicUsize::new(0));
    let profiles = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(handler);
    for connection in connections {
        let Ok(stream) = connection else { continue };
        let Some(reader_permit) = Permit::acquire(&readers, MAX_CONCURRENT_IPC_READERS) else {
            // Never execute a request or wait for a socket on the accept loop.
            drop(stream);
            continue;
        };
        let profiles = profiles.clone();
        let handler = handler.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle_client_stream(stream, reader_permit, &profiles, &*handler) {
                eprintln!("Monarch IPC request failed: {error}");
            }
        });
    }
}

fn handle_client_stream(
    mut stream: TcpStream,
    reader_permit: Permit,
    profiles: &Arc<AtomicUsize>,
    handler: &impl Fn(IpcRequest) -> Result<(), String>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|err| err.to_string())?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_BYTES + 1))
        .read_line(&mut line)
        .map_err(|err| err.to_string())?;
    if line.len() as u64 > MAX_REQUEST_BYTES || !line.ends_with('\n') {
        return Err("invalid or oversized IPC request".into());
    }
    let request: IpcRequest = serde_json::from_str(line.trim()).map_err(|err| err.to_string())?;
    let mut profile_permit = None;
    let result = if matches!(request, IpcRequest::ApplyProfile { .. }) {
        profile_permit = Permit::acquire(profiles, MAX_CONCURRENT_PROFILE_REQUESTS);
        if profile_permit.is_some() {
            // A slow apply consumes only a profile slot. Readers remain available for Show.
            drop(reader_permit);
            handler(request)
        } else {
            Err("Monarch is busy applying a profile; try again when it finishes".into())
        }
    } else {
        handler(request)
    };
    let response = IpcResponse {
        ok: result.is_ok(),
        error: result.err(),
    };
    let mut bytes = serde_json::to_vec(&response).map_err(|err| err.to_string())?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|err| err.to_string())?;
    drop(profile_permit);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Condvar, Mutex};

    fn request(address: std::net::SocketAddr, payload: &str) -> IpcResponse {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(socket, "{payload}").unwrap();
        let mut response = String::new();
        BufReader::new(socket).read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn saturated_profiles_do_not_block_window_requests_or_the_accept_loop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let release = gate.clone();
        let (started, receiving) = mpsc::channel();
        let server = std::thread::spawn(move || {
            serve_connections(
                listener
                    .incoming()
                    .take(MAX_CONCURRENT_PROFILE_REQUESTS + 2),
                move |request| {
                    if matches!(request, IpcRequest::ApplyProfile { .. }) {
                        started.send(()).unwrap();
                        let (lock, wake) = &*gate;
                        let _guard = wake
                            .wait_while(lock.lock().unwrap(), |released| !*released)
                            .unwrap();
                    }
                    Ok(())
                },
            );
        });
        let clients: Vec<_> = (0..MAX_CONCURRENT_PROFILE_REQUESTS)
            .map(|_| {
                std::thread::spawn(move || {
                    request(address, r#"{"type":"apply_profile","name":"test"}"#)
                })
            })
            .collect();
        for _ in 0..MAX_CONCURRENT_PROFILE_REQUESTS {
            receiving.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let busy = request(address, r#"{"type":"apply_profile","name":"extra"}"#);
        let shown = request(address, r#"{"type":"show_window"}"#);
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        assert!(!busy.ok);
        assert!(busy.error.unwrap().contains("busy"));
        assert!(shown.ok);
        for client in clients {
            assert!(client.join().unwrap().ok);
        }
        server.join().unwrap();
    }

    #[test]
    fn permits_release_capacity_on_drop() {
        let count = Arc::new(AtomicUsize::new(0));
        let slot = Permit::acquire(&count, 1).unwrap();
        assert!(Permit::acquire(&count, 1).is_none());
        drop(slot);
        assert!(Permit::acquire(&count, 1).is_some());
    }
}
