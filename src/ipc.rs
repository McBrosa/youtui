use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

const COMMAND_REPLY_TIMEOUT: Duration = Duration::from_millis(500);
const PROPERTY_REPLY_TIMEOUT: Duration = Duration::from_millis(150);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);

pub struct IpcClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_request_id: u64,
    events: VecDeque<Value>,
}

impl IpcClient {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).with_context(|| {
            format!("Failed to connect to IPC socket {}", socket_path.display())
        })?;
        Self::from_stream(stream)
    }

    pub(crate) fn from_stream(stream: UnixStream) -> Result<Self> {
        stream.set_read_timeout(Some(COMMAND_REPLY_TIMEOUT))?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let reader_stream = stream
            .try_clone()
            .context("Failed to clone IPC socket for reading")?;

        Ok(Self {
            reader: BufReader::new(reader_stream),
            writer: stream,
            next_request_id: 1,
            events: VecDeque::new(),
        })
    }

    pub fn send_command(&mut self, command: &[&str]) -> Result<()> {
        self.send_command_with_data(command).map(|_| ())
    }

    pub fn send_command_with_data(&mut self, command: &[&str]) -> Result<Option<Value>> {
        self.set_read_timeout(COMMAND_REPLY_TIMEOUT)?;
        let response = self
            .execute_commands(&[command])?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("mpv did not reply to the command"))?;
        ensure_success(&response)?;
        Ok(response.get("data").cloned())
    }

    #[allow(dead_code)]
    pub fn get_property(&mut self, property: &str) -> Result<Value> {
        let values = self.get_properties(&[property])?;
        values
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| anyhow!("Property unavailable: {property}"))
    }

    /// Fetch several properties in a single write/read batch. Each missing or
    /// unavailable property is returned as `None`, while transport and JSON
    /// errors still fail the whole request.
    pub fn get_properties(&mut self, properties: &[&str]) -> Result<Vec<Option<Value>>> {
        if properties.is_empty() {
            return Ok(Vec::new());
        }

        // Status polling runs on the UI thread. Keep its wait bounded; mpv can
        // temporarily stop answering while yt-dlp resolves a new stream.
        self.set_read_timeout(PROPERTY_REPLY_TIMEOUT)?;

        let commands: Vec<[&str; 2]> = properties
            .iter()
            .map(|property| ["get_property", *property])
            .collect();
        let command_refs: Vec<&[&str]> =
            commands.iter().map(|command| command.as_slice()).collect();
        let responses = self.execute_commands(&command_refs)?;

        Ok(responses
            .into_iter()
            .map(|response| {
                if response.get("error").and_then(Value::as_str) == Some("success") {
                    response.get("data").cloned()
                } else {
                    None
                }
            })
            .collect())
    }

    fn execute_commands(&mut self, commands: &[&[&str]]) -> Result<Vec<Value>> {
        if commands.is_empty() {
            return Ok(Vec::new());
        }

        let mut payload = Vec::new();
        let mut request_indexes = HashMap::with_capacity(commands.len());

        for (index, command) in commands.iter().enumerate() {
            let request_id = self.allocate_request_id();
            request_indexes.insert(request_id, index);
            serde_json::to_writer(
                &mut payload,
                &json!({ "command": command, "request_id": request_id }),
            )?;
            payload.push(b'\n');
        }

        self.writer
            .write_all(&payload)
            .context("Failed to write command to mpv IPC socket")?;

        let mut responses = vec![None; commands.len()];
        let mut remaining = commands.len();

        while remaining > 0 {
            let mut line = String::new();
            let bytes_read = self
                .reader
                .read_line(&mut line)
                .context("Failed to read response from mpv IPC socket")?;
            if bytes_read == 0 {
                bail!("mpv closed the IPC socket before replying");
            }

            let response: Value = serde_json::from_str(&line)
                .with_context(|| format!("Invalid JSON response from mpv IPC: {}", line.trim()))?;
            let Some(request_id) = response.get("request_id").and_then(Value::as_u64) else {
                // mpv events do not have a request ID. Preserve them while
                // waiting for command replies so short tracks cannot finish
                // entirely between two status polls.
                if response.get("event").is_some() {
                    self.events.push_back(response);
                }
                continue;
            };
            let Some(&index) = request_indexes.get(&request_id) else {
                // A delayed response from an older request must not be mistaken
                // for one of the replies in the current batch.
                continue;
            };

            if responses[index].is_none() {
                responses[index] = Some(response);
                remaining -= 1;
            }
        }

        responses
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!("mpv IPC response batch was incomplete"))
    }

    fn allocate_request_id(&mut self) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id = if self.next_request_id >= i64::MAX as u64 {
            1
        } else {
            self.next_request_id + 1
        };
        request_id
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> Result<()> {
        self.reader
            .get_mut()
            .set_read_timeout(Some(timeout))
            .context("Failed to configure mpv IPC read timeout")?;
        Ok(())
    }

    pub fn is_read_timeout(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                )
            })
        })
    }

    pub fn take_events(&mut self) -> Vec<Value> {
        self.events.drain(..).collect()
    }
}

fn ensure_success(response: &Value) -> Result<()> {
    match response.get("error").and_then(Value::as_str) {
        Some("success") => Ok(()),
        Some(error) => bail!("mpv command failed: {error}"),
        None => bail!("mpv IPC response did not contain a status"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn read_request(reader: &mut BufReader<UnixStream>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn reply(stream: &mut UnixStream, request_id: u64, error: &str, data: Value) {
        let response = json!({
            "request_id": request_id,
            "error": error,
            "data": data,
        });
        writeln!(stream, "{response}").unwrap();
    }

    #[test]
    fn command_reply_is_consumed_before_next_property_read() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server_reader = server_stream.try_clone().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(server_reader);

            let command = read_request(&mut reader);
            assert_eq!(
                command["command"],
                json!(["loadfile", "video-url", "replace"])
            );
            reply(
                &mut server_stream,
                command["request_id"].as_u64().unwrap(),
                "success",
                json!({ "playlist_entry_id": 7 }),
            );

            let property = read_request(&mut reader);
            assert_eq!(property["command"], json!(["get_property", "duration"]));
            reply(
                &mut server_stream,
                property["request_id"].as_u64().unwrap(),
                "success",
                json!(123.5),
            );
        });

        let mut client = IpcClient::from_stream(client_stream).unwrap();
        let data = client
            .send_command_with_data(&["loadfile", "video-url", "replace"])
            .unwrap();
        assert_eq!(data, Some(json!({ "playlist_entry_id": 7 })));
        assert_eq!(client.get_property("duration").unwrap(), json!(123.5));
        server.join().unwrap();
    }

    #[test]
    fn batched_properties_are_correlated_when_replies_arrive_out_of_order() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server_reader = server_stream.try_clone().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(server_reader);
            let duration = read_request(&mut reader);
            let paused = read_request(&mut reader);

            // An unrelated event and reversed replies exercise request correlation.
            writeln!(server_stream, "{}", json!({ "event": "idle" })).unwrap();
            reply(
                &mut server_stream,
                paused["request_id"].as_u64().unwrap(),
                "success",
                json!(true),
            );
            reply(
                &mut server_stream,
                duration["request_id"].as_u64().unwrap(),
                "success",
                json!(42.0),
            );
        });

        let mut client = IpcClient::from_stream(client_stream).unwrap();
        let values = client.get_properties(&["duration", "pause"]).unwrap();
        assert_eq!(values, vec![Some(json!(42.0)), Some(json!(true))]);
        assert_eq!(client.take_events(), vec![json!({ "event": "idle" })]);
        server.join().unwrap();
    }

    #[test]
    fn unavailable_property_does_not_discard_other_batched_values() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server_reader = server_stream.try_clone().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(server_reader);
            let missing = read_request(&mut reader);
            let volume = read_request(&mut reader);
            reply(
                &mut server_stream,
                missing["request_id"].as_u64().unwrap(),
                "property unavailable",
                Value::Null,
            );
            reply(
                &mut server_stream,
                volume["request_id"].as_u64().unwrap(),
                "success",
                json!(75.0),
            );
        });

        let mut client = IpcClient::from_stream(client_stream).unwrap();
        let values = client.get_properties(&["duration", "volume"]).unwrap();
        assert_eq!(values, vec![None, Some(json!(75.0))]);
        server.join().unwrap();
    }

    #[test]
    fn delayed_property_reply_is_classified_as_a_transient_timeout() {
        let (client_stream, mut server_stream) = UnixStream::pair().unwrap();
        let server_reader = server_stream.try_clone().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(server_reader);
            let request = read_request(&mut reader);
            thread::sleep(PROPERTY_REPLY_TIMEOUT + Duration::from_millis(50));
            reply(
                &mut server_stream,
                request["request_id"].as_u64().unwrap(),
                "success",
                json!(12.0),
            );
        });

        let mut client = IpcClient::from_stream(client_stream).unwrap();
        let error = client.get_properties(&["time-pos"]).unwrap_err();

        assert!(IpcClient::is_read_timeout(&error));
        server.join().unwrap();
    }
}
