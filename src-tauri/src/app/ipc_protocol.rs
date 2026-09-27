use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum IpcRequest {
    ApplyProfile {
        name: String,
    },
    #[serde(alias = "show_window")]
    ShowMainWindow,
}
#[cfg(any(windows, test))]
#[derive(Serialize, Deserialize)]
pub(crate) struct IpcResponse {
    pub(crate) request_id: u64,
    pub(crate) completed: bool,
    pub(crate) error: Option<String>,
}

#[cfg(windows)]
pub(crate) use framing::*;

#[cfg(any(windows, test))]
mod framing {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::time::timeout;
    pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(3);
    pub(crate) const COMPLETION_TIMEOUT: Duration = Duration::from_secs(90);
    const MAX_FRAME: u64 = 8192;
    pub(crate) async fn read_frame<T: for<'de> Deserialize<'de>, R: AsyncBufRead + Unpin>(
        reader: &mut R,
    ) -> Result<T, String> {
        let mut bytes = Vec::new();
        reader
            .take(MAX_FRAME + 1)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FRAME || bytes.last() != Some(&b'\n') {
            return Err("invalid IPC frame".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }
    pub(crate) async fn write_frame<T: Serialize, W: AsyncWrite + Unpin>(
        writer: &mut W,
        value: &T,
    ) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_FRAME {
            return Err("IPC frame exceeds 8 KiB".into());
        }
        timeout(IO_TIMEOUT, writer.write_all(&bytes))
            .await
            .map_err(|_| "IPC write timed out".to_string())?
            .map_err(|e| e.to_string())
    }

    pub(crate) async fn await_response<R: AsyncBufRead + Unpin>(
        reader: &mut R,
        acknowledgement_timeout: Duration,
        completion_timeout: Duration,
    ) -> Result<(), String> {
        let accepted: IpcResponse = timeout(acknowledgement_timeout, read_frame(reader))
            .await
            .map_err(|_| {
                "IPC acknowledgement timed out; submission outcome is unknown".to_string()
            })?
            .map_err(|error| {
                format!("IPC acknowledgement failed; submission outcome is unknown: {error}")
            })?;
        if let Some(error) = accepted.error {
            return Err(error);
        }
        if accepted.completed {
            return Ok(());
        }
        let completed: IpcResponse = timeout(completion_timeout, read_frame(reader))
            .await
            .map_err(|_| {
                format!(
                    "request {} was accepted; completion is unknown. Do not retry automatically",
                    accepted.request_id
                )
            })?
            .map_err(|error| {
                format!(
                    "request {} was accepted; completion is unknown: {error}",
                    accepted.request_id
                )
            })?;
        if completed.request_id != accepted.request_id || !completed.completed {
            return Err("invalid IPC completion".into());
        }
        completed.error.map_or(Ok(()), Err)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::BufReader;
        fn run(future: impl std::future::Future<Output = ()>) {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(future);
        }

        #[test]
        fn frame_reader_preserves_buffered_following_frames() {
            run(async {
                let bytes = b"{\"type\":\"show_main_window\"}\n{\"type\":\"apply_profile\",\"name\":\"Desk\"}\n";
                let mut reader = BufReader::new(&bytes[..]);
                assert!(matches!(
                    read_frame::<IpcRequest, _>(&mut reader).await.unwrap(),
                    IpcRequest::ShowMainWindow
                ));
                assert!(
                    matches!(read_frame::<IpcRequest, _>(&mut reader).await.unwrap(), IpcRequest::ApplyProfile { name } if name == "Desk")
                );
            });
        }

        #[test]
        fn malformed_oversized_and_unterminated_frames_are_rejected() {
            run(async {
                for bytes in [
                    b"invalid\n".to_vec(),
                    b"{}".to_vec(),
                    vec![b' '; MAX_FRAME as usize + 1],
                ] {
                    assert!(read_frame::<IpcRequest, _>(&mut BufReader::new(&bytes[..]))
                        .await
                        .is_err());
                }
                let (_writer, reader) = tokio::io::duplex(64);
                assert!(timeout(
                    Duration::from_millis(20),
                    read_frame::<IpcRequest, _>(&mut BufReader::new(reader))
                )
                .await
                .is_err());
            });
        }

        #[test]
        fn acceptance_and_completion_have_separate_deadlines() {
            run(async {
                let (mut writer, reader) = tokio::io::duplex(1024);
                write_frame(
                    &mut writer,
                    &IpcResponse {
                        request_id: 7,
                        completed: false,
                        error: None,
                    },
                )
                .await
                .unwrap();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    write_frame(
                        &mut writer,
                        &IpcResponse {
                            request_id: 7,
                            completed: true,
                            error: None,
                        },
                    )
                    .await
                    .unwrap();
                });
                assert!(await_response(
                    &mut BufReader::new(reader),
                    Duration::from_millis(20),
                    Duration::from_secs(2)
                )
                .await
                .is_ok());
            });
        }

        #[test]
        fn disconnect_after_acceptance_reports_unknown_outcome() {
            run(async {
                let bytes = b"{\"request_id\":42,\"completed\":false,\"error\":null}\n";
                let error = await_response(
                    &mut BufReader::new(&bytes[..]),
                    IO_TIMEOUT,
                    COMPLETION_TIMEOUT,
                )
                .await
                .unwrap_err();
                assert!(error.contains("42"));
                assert!(error.contains("completion is unknown"));
            });
        }
    }
}
