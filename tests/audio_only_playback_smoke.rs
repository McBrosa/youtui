#![cfg(unix)]

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const ROWS: u16 = 36;
const COLS: u16 = 120;
const WAIT_TIMEOUT: Duration = Duration::from_secs(8);
const RESULT_TITLE: &str = "Integration Audio Smoke Track";
const VIDEO_ID: &str = "integration-audio-id";

#[test]
fn audio_only_playback_survives_a_partial_ipc_timeout() {
    let fixture = SmokeFixture::new();
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"integration audio smoke\r");
    youtui.wait_for_screen(RESULT_TITLE, WAIT_TIMEOUT);

    let yt_dlp_args = wait_for_lines(&fixture.yt_dlp_args, WAIT_TIMEOUT);
    assert!(yt_dlp_args.iter().any(|arg| arg == "--lazy-playlist"));
    assert!(yt_dlp_args.iter().any(|arg| arg == "--dump-json"));
    assert!(
        yt_dlp_args
            .iter()
            .any(|arg| arg == "ytsearch500:integration audio smoke")
    );

    // Start watching before Enter so even a heavily loaded CI runner can bind
    // the socket well inside PlayerManager's connection deadline.
    let server_args = fixture.mpv_args.clone();
    let server = thread::spawn(move || {
        let args = wait_for_lines(&server_args, WAIT_TIMEOUT);
        let socket_path = ipc_socket_path(&args)?;
        let result = run_fake_mpv(&socket_path);
        if let Err(error) = &result {
            eprintln!("fake mpv server failed: {error}");
        }
        result
    });

    youtui.write_all(b"\r");
    let mpv_args = wait_for_lines(&fixture.mpv_args, WAIT_TIMEOUT);
    assert!(mpv_args.iter().any(|arg| arg == "--idle"));
    assert!(mpv_args.iter().any(|arg| arg == "--no-video"));
    assert!(
        mpv_args
            .iter()
            .any(|arg| arg == "--ytdl-format=bestaudio/best")
    );

    youtui.wait_for_screen("0:12", WAIT_TIMEOUT);
    youtui.wait_for_screen("1:40", WAIT_TIMEOUT);
    youtui.wait_for_screen("77%", WAIT_TIMEOUT);

    let screen = youtui.screen();
    let transcript = youtui.transcript();
    assert!(!screen.contains("Playback connection lost"), "{screen}");
    assert!(
        !transcript.contains("Playback connection lost"),
        "{transcript}"
    );

    youtui.write_all(b"q");
    let status = youtui.wait_for_exit(WAIT_TIMEOUT);
    assert!(status.success(), "youtui exited with {status}");
    youtui.finish_reader();

    let report = server
        .join()
        .expect("fake mpv server panicked")
        .expect("fake mpv server failed");
    assert_eq!(
        report.connections, 2,
        "IPC was not reconnected exactly once"
    );
    assert_eq!(report.loadfile_count, 1, "track was unexpectedly reloaded");
    assert_eq!(
        report.loaded_url,
        format!("https://www.youtube.com/watch?v={VIDEO_ID}")
    );
}

struct SmokeFixture {
    _root: tempfile::TempDir,
    _runtime_root: tempfile::TempDir,
    root: PathBuf,
    fake_bin: PathBuf,
    home: PathBuf,
    xdg_config: PathBuf,
    runtime_tmp: PathBuf,
    mpv_args: PathBuf,
    yt_dlp_args: PathBuf,
}

impl SmokeFixture {
    fn new() -> Self {
        // Keep executable shims off /tmp because hardened Linux systems may
        // mount it noexec. The compiled binary already proves target is executable.
        let fixture_parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        fs::create_dir_all(&fixture_parent).expect("failed to create fixture parent");
        let root = tempfile::Builder::new()
            .prefix("yts-")
            .tempdir_in(fixture_parent)
            .expect("failed to create smoke-test directory");
        // Keep only TMPDIR short: macOS Unix-domain socket paths have a small
        // platform limit, and PlayerManager creates another directory below it.
        let runtime_root = tempfile::Builder::new()
            .prefix("ytr-")
            .tempdir_in("/tmp")
            .expect("failed to create short runtime directory");
        let root_path = root.path().to_path_buf();
        let fake_bin = root_path.join("bin");
        let home = root_path.join("home");
        let xdg_config = root_path.join("config");
        let runtime_tmp = runtime_root.path().to_path_buf();
        let mpv_args = root_path.join("mpv.args");
        let yt_dlp_args = root_path.join("yt-dlp.args");

        for directory in [&fake_bin, &home, &xdg_config] {
            fs::create_dir_all(directory).expect("failed to create fixture directory");
        }

        let config = "audio_only = true\n";
        write_config(&xdg_config.join("youtui/config.toml"), config);
        write_config(
            &home.join("Library/Application Support/youtui/config.toml"),
            config,
        );

        write_executable(
            &fake_bin.join("yt-dlp"),
            r#"#!/bin/sh
set -eu
tmp="${YOUTUI_SMOKE_YTDLP_ARGS}.tmp.$$"
printf '%s\n' "$@" > "$tmp"
mv "$tmp" "$YOUTUI_SMOKE_YTDLP_ARGS"
printf '%s\n' '{"id":"integration-audio-id","title":"Integration Audio Smoke Track","duration":240,"duration_string":"4:00","channel":"Smoke Channel","view_count":1234}'
"#,
        );
        write_executable(
            &fake_bin.join("mpv"),
            r#"#!/bin/sh
set -eu
tmp="${YOUTUI_SMOKE_MPV_ARGS}.tmp.$$"
printf '%s\n' "$@" > "$tmp"
mv "$tmp" "$YOUTUI_SMOKE_MPV_ARGS"
exec sleep 30
"#,
        );

        Self {
            _root: root,
            _runtime_root: runtime_root,
            root: root_path,
            fake_bin,
            home,
            xdg_config,
            runtime_tmp,
            mpv_args,
            yt_dlp_args,
        }
    }

    fn spawn_youtui(&self) -> PtyProcess {
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.fake_bin.clone()).chain(std::env::split_paths(&inherited_path)),
        )
        .expect("failed to construct fixture PATH");

        let mut command = Command::new(env!("CARGO_BIN_EXE_youtui"));
        command
            .current_dir(&self.root)
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("TMPDIR", &self.runtime_tmp)
            .env("TERM", "xterm-256color")
            .env("YOUTUI_SMOKE_MPV_ARGS", &self.mpv_args)
            .env("YOUTUI_SMOKE_YTDLP_ARGS", &self.yt_dlp_args);

        PtyProcess::spawn(command)
    }
}

fn write_config(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("config path has no parent"))
        .expect("failed to create config directory");
    fs::write(path, contents).expect("failed to write isolated config");
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("failed to write fake executable");
    let mut permissions = fs::metadata(path)
        .expect("failed to stat fake executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to mark fake executable as executable");
}

fn wait_for_lines(path: &Path, timeout: Duration) -> Vec<String> {
    let started = Instant::now();
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            return contents.lines().map(str::to_owned).collect();
        }
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn ipc_socket_path(args: &[String]) -> Result<PathBuf, String> {
    args.iter()
        .find_map(|arg| arg.strip_prefix("--input-ipc-server="))
        .map(PathBuf::from)
        .ok_or_else(|| "fake mpv did not receive an IPC socket path".to_string())
}

struct PtyProcess {
    child: Child,
    process_group: i32,
    finished_cleanly: bool,
    writer: Option<File>,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

impl PtyProcess {
    fn spawn(mut command: Command) -> Self {
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let mut window = libc::winsize {
            ws_row: ROWS,
            ws_col: COLS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty initializes both file descriptors on success. They
        // are immediately wrapped in owned Files exactly once below.
        let result = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut window,
            )
        };
        assert_eq!(result, 0, "openpty failed: {}", io::Error::last_os_error());

        // SAFETY: both descriptors are valid and uniquely owned after openpty.
        let master = unsafe { File::from_raw_fd(master_fd) };
        // SAFETY: see above.
        let slave = unsafe { File::from_raw_fd(slave_fd) };
        command
            .stdin(Stdio::from(
                slave.try_clone().expect("failed to clone PTY slave"),
            ))
            .stdout(Stdio::from(
                slave.try_clone().expect("failed to clone PTY slave"),
            ))
            .stderr(Stdio::from(slave));
        command.process_group(0);

        let child = command.spawn().expect("failed to launch youtui in PTY");
        let process_group = i32::try_from(child.id()).expect("child PID did not fit in i32");
        let mut reader_file = master.try_clone().expect("failed to clone PTY master");
        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let reader = thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader_file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => reader_output
                        .lock()
                        .expect("PTY output lock poisoned")
                        .extend_from_slice(&buffer[..size]),
                    // Linux PTYs commonly return EIO when the slave closes.
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                    Err(_) => break,
                }
            }
        });

        Self {
            child,
            process_group,
            finished_cleanly: false,
            writer: Some(master),
            output,
            reader: Some(reader),
        }
    }

    fn write_all(&mut self, input: &[u8]) {
        self.writer
            .as_mut()
            .expect("PTY writer is closed")
            .write_all(input)
            .expect("failed to write PTY input");
    }

    fn screen(&self) -> String {
        let output = self.output.lock().expect("PTY output lock poisoned");
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        parser.process(&output);
        parser.screen().contents()
    }

    fn transcript(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().expect("PTY output lock poisoned")).into_owned()
    }

    fn wait_for_screen(&mut self, needle: &str, timeout: Duration) {
        let started = Instant::now();
        loop {
            let screen = self.screen();
            if screen.contains(needle) {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("failed to inspect youtui") {
                panic!("youtui exited with {status} while waiting for {needle:?}\n{screen}");
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for {needle:?}\n{screen}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("failed to inspect youtui") {
                self.finished_cleanly = status.success();
                return status;
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for youtui to exit"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish_reader(&mut self) {
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            reader.join().expect("PTY reader panicked");
        }
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        if !self.finished_cleanly {
            // SAFETY: process_group is the positive PID returned for the child
            // we launched with process_group(0). A negative PID targets only
            // that group, including the fake mpv if youtui failed before Drop.
            unsafe {
                libc::kill(-self.process_group, libc::SIGKILL);
            }
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.finish_reader();
    }
}

#[derive(Debug)]
struct ServerReport {
    connections: usize,
    loadfile_count: usize,
    loaded_url: String,
}

fn run_fake_mpv(socket_path: &Path) -> Result<ServerReport, String> {
    let listener = UnixListener::bind(socket_path)
        .map_err(|error| format!("failed to bind {}: {error}", socket_path.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;

    let first =
        accept_until(&listener, WAIT_TIMEOUT).map_err(|error| format!("first accept: {error}"))?;
    let mut first = BufReader::new(first);
    let loadfile = read_request(&mut first, WAIT_TIMEOUT)
        .map_err(|error| format!("loadfile request: {error}"))?;
    let command = command_parts(&loadfile)?;
    if command.first().and_then(Value::as_str) != Some("loadfile") {
        return Err(format!("expected loadfile, received {loadfile}"));
    }
    let loaded_url = command
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("loadfile URL missing from {loadfile}"))?
        .to_owned();
    if command.get(2).and_then(Value::as_str) != Some("replace") {
        return Err(format!("loadfile did not replace the playlist: {loadfile}"));
    }
    reply(
        first.get_mut(),
        request_id(&loadfile)?,
        json!({ "playlist_entry_id": 42 }),
    )?;

    let first_batch = read_property_batch(&mut first)
        .map_err(|error| format!("first property batch: {error}"))?;
    let partial = format!(
        "{{\"request_id\":{},\"error\":\"success\",\"data\":12",
        request_id(&first_batch[0])?
    );
    first
        .get_mut()
        .write_all(partial.as_bytes())
        .map_err(|error| error.to_string())?;
    first.get_mut().flush().map_err(|error| error.to_string())?;
    wait_for_disconnect(&mut first, WAIT_TIMEOUT)
        .map_err(|error| format!("first disconnect: {error}"))?;

    let second =
        accept_until(&listener, WAIT_TIMEOUT).map_err(|error| format!("second accept: {error}"))?;
    let mut second = BufReader::new(second);
    let second_batch = read_property_batch(&mut second)
        .map_err(|error| format!("second property batch: {error}"))?;
    second
        .get_mut()
        .write_all(b"{\"event\":\"file-loaded\"}\n")
        .map_err(|error| error.to_string())?;
    reply_to_properties(second.get_mut(), &second_batch)?;

    let mut loadfile_count = 1;
    second
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|error| error.to_string())?;
    loop {
        let mut line = String::new();
        match second.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let request: Value = serde_json::from_str(&line)
                    .map_err(|error| format!("invalid follow-up request {line:?}: {error}"))?;
                let command = command_parts(&request)?;
                if command.first().and_then(Value::as_str) == Some("loadfile") {
                    loadfile_count += 1;
                }
                if command.first().and_then(Value::as_str) == Some("get_property") {
                    reply_to_property(second.get_mut(), &request)?;
                } else {
                    reply(second.get_mut(), request_id(&request)?, Value::Null)?;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    Ok(ServerReport {
        connections: 2,
        loadfile_count,
        loaded_url,
    })
}

fn accept_until(listener: &UnixListener, timeout: Duration) -> Result<UnixStream, String> {
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets inherit O_NONBLOCK on some Unix variants.
                stream
                    .set_nonblocking(false)
                    .map_err(|error| error.to_string())?;
                return Ok(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if started.elapsed() >= timeout {
                    return Err("timed out waiting for mpv IPC connection".to_string());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn read_request(reader: &mut BufReader<UnixStream>, timeout: Duration) -> Result<Value, String> {
    reader
        .get_mut()
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    let mut line = String::new();
    let size = reader
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if size == 0 {
        return Err("IPC client disconnected before sending a request".to_string());
    }
    serde_json::from_str(&line).map_err(|error| format!("invalid request {line:?}: {error}"))
}

fn read_property_batch(reader: &mut BufReader<UnixStream>) -> Result<Vec<Value>, String> {
    let mut requests = Vec::with_capacity(5);
    for _ in 0..5 {
        let request = read_request(reader, WAIT_TIMEOUT)?;
        let command = command_parts(&request)?;
        if command.first().and_then(Value::as_str) != Some("get_property") {
            return Err(format!("expected get_property, received {request}"));
        }
        requests.push(request);
    }
    Ok(requests)
}

fn wait_for_disconnect(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
) -> Result<(), String> {
    reader
        .get_mut()
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    let mut byte = [0_u8; 1];
    match reader.read(&mut byte) {
        Ok(0) => Ok(()),
        Ok(_) => Err("IPC client sent unexpected data after the timed-out batch".to_string()),
        Err(error) => Err(format!(
            "IPC client did not disconnect after timeout: {error}"
        )),
    }
}

fn reply_to_properties(stream: &mut UnixStream, requests: &[Value]) -> Result<(), String> {
    // Reverse reply order to prove request IDs, rather than arrival order,
    // correlate each property value.
    for request in requests.iter().rev() {
        reply_to_property(stream, request)?;
    }
    Ok(())
}

fn reply_to_property(stream: &mut UnixStream, request: &Value) -> Result<(), String> {
    let command = command_parts(request)?;
    let property = command
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("property name missing from {request}"))?;
    let data = match property {
        "time-pos" => json!(12.0),
        "duration" => json!(100.0),
        "pause" => json!(false),
        "volume" => json!(77.0),
        "eof-reached" => json!(false),
        _ => return Err(format!("unexpected property {property}")),
    };
    reply(stream, request_id(request)?, data)
}

fn reply(stream: &mut UnixStream, request_id: u64, data: Value) -> Result<(), String> {
    serde_json::to_writer(
        &mut *stream,
        &json!({ "request_id": request_id, "error": "success", "data": data }),
    )
    .map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn request_id(request: &Value) -> Result<u64, String> {
    request
        .get("request_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("request ID missing from {request}"))
}

fn command_parts(request: &Value) -> Result<&Vec<Value>, String> {
    request
        .get("command")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("command missing from {request}"))
}
