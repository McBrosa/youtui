use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;
use anyhow::{Context, Result, bail};
use serde_json::{json, Value};

pub struct IpcClient {
    stream: UnixStream,
}

impl IpcClient {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .context("Failed to connect to IPC socket")?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self { stream })
    }

    pub fn send_command(&mut self, command: &[&str]) -> Result<()> {
        let json_cmd = json!({ "command": command });
        let mut cmd_str = serde_json::to_string(&json_cmd)?;
        cmd_str.push('\n');
        self.stream.write_all(cmd_str.as_bytes())?;
        Ok(())
    }

    pub fn get_property(&mut self, property: &str) -> Result<Value> {
        self.send_command(&["get_property", property])?;
        let mut reader = BufReader::new(&self.stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        let json: Value = serde_json::from_str(&response)?;

        if json["error"].as_str() != Some("success") {
            bail!("Property unavailable: {}", property);
        }

        Ok(json["data"].clone())
    }
}
