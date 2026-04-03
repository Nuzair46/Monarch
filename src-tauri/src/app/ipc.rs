use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

use crate::app::events;

const IPC_BIND_ADDR: &str = "127.0.0.1:42197";
const IPC_IO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcRequest {
    ApplyProfile { name: String },
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

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Err(err) = handle_client_stream(&app, stream) {
                        eprintln!("Monarch IPC request failed: {err}");
                    }
                }
                Err(err) => {
                    eprintln!("Monarch IPC incoming connection failed: {err}");
                }
            }
        }
    });
}

fn handle_client_stream<R: Runtime>(
    app: &AppHandle<R>,
    mut stream: TcpStream,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|err| format!("failed to set client read timeout: {err}"))?;
    stream
        .set_write_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|err| format!("failed to set client write timeout: {err}"))?;

    let mut request_line = String::new();
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|err| format!("failed to clone client stream: {err}"))?,
    );
    reader
        .read_line(&mut request_line)
        .map_err(|err| format!("failed to read IPC request: {err}"))?;

    let request = serde_json::from_str::<IpcRequest>(request_line.trim())
        .map_err(|err| format!("invalid IPC request payload: {err}"))?;

    let result = match request {
        IpcRequest::ApplyProfile { name } => {
            events::apply_profile_external_action_result(app, &name)
        }
        IpcRequest::ShowMainWindow => {
            events::show_main_window(app);
            Ok(())
        }
    };

    let response = match result {
        Ok(()) => IpcResponse {
            ok: true,
            error: None,
        },
        Err(err) => IpcResponse {
            ok: false,
            error: Some(err),
        },
    };

    let mut response_bytes = serde_json::to_vec(&response)
        .map_err(|err| format!("failed to encode IPC response: {err}"))?;
    response_bytes.push(b'\n');
    stream
        .write_all(&response_bytes)
        .map_err(|err| format!("failed to send IPC response: {err}"))?;
    Ok(())
}
