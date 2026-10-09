#![cfg(unix)]

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn web_launcher_runs_without_a_terminal_and_stops_its_server() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let script = bin.join("youtui_web");
    fs::write(&script, FAKE_SERVER).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let port = free_port();
    let marker = directory.path().join("stopped");
    let config_marker = directory.path().join("config-path");
    let mut launcher = Command::new(env!("CARGO_BIN_EXE_youtui"))
        .args(["--web", "--no-open", "--port", &port.to_string()])
        .env("YOUTUI_WEB_RELEASE", directory.path())
        .env("YOUTUI_TEST_STOP_MARKER", &marker)
        .env("YOUTUI_TEST_CONFIG_MARKER", &config_marker)
        .env("YOUTUI_CONFIG_PATH", "relative-web-test-config.toml")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "web server did not start"
        );
        assert!(
            launcher.try_wait().unwrap().is_none(),
            "launcher exited before readiness"
        );
        thread::sleep(Duration::from_millis(50));
    }
    // Let the launcher finish its authenticated readiness probe.
    thread::sleep(Duration::from_millis(300));
    let pid = i32::try_from(launcher.id()).unwrap();
    // SAFETY: the PID is this test's live child, and SIGINT requests orderly shutdown.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    let stopped = Instant::now();
    loop {
        if let Some(status) = launcher.try_wait().unwrap() {
            assert!(status.success(), "launcher failed: {status}");
            break;
        }
        if stopped.elapsed() >= Duration::from_secs(10) {
            let _ = launcher.kill();
            panic!("launcher did not stop");
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::read_to_string(marker).unwrap(), "stopped");
    assert_eq!(
        fs::read_to_string(config_marker).unwrap(),
        std::env::current_dir()
            .unwrap()
            .join("relative-web-test-config.toml")
            .to_string_lossy()
    );
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn web_launcher_reports_an_occupied_port_before_starting_phoenix() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let output = Command::new(env!("CARGO_BIN_EXE_youtui"))
        .args(["--web", "--no-open", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("choose another port"));
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

const FAKE_SERVER: &str = r#"#!/usr/bin/python3
import http.server
import json
import os
import signal
import sys

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get('x-youtui-token') != os.environ['YOUTUI_WEB_TOKEN']:
            self.send_error(403)
            return
        body = json.dumps({'ready': True}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

def stop(signum, frame):
    with open(os.environ['YOUTUI_TEST_STOP_MARKER'], 'w') as marker:
        marker.write('stopped')
    sys.exit(0)

signal.signal(signal.SIGTERM, stop)
with open(os.environ['YOUTUI_TEST_CONFIG_MARKER'], 'w') as marker:
    marker.write(os.environ['YOUTUI_CONFIG_PATH'])
http.server.HTTPServer(('127.0.0.1', int(os.environ['YOUTUI_PORT'])), Handler).serve_forever()
"#;
