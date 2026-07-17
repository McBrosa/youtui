#![cfg(unix)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ROWS: u16 = 36;
const COLS: u16 = 120;
const WAIT_TIMEOUT: Duration = Duration::from_secs(8);

#[test]
fn first_page_is_visible_while_the_same_search_process_prefetches() {
    let fixture = SearchFixture::new(20, false);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"stream first page\r");
    wait_for_path(&fixture.control.join("first-page-emitted"), WAIT_TIMEOUT);

    // The fake remains blocked after writing the first page. Seeing a result
    // here proves youtui consumes complete JSON lines instead of waiting for
    // yt-dlp to exit and collecting its entire stdout.
    youtui.wait_for_screen("Streaming Result 01", WAIT_TIMEOUT);
    assert!(!fixture.control.join("released").exists());
    assert_eq!(fixture.invocations().len(), 1);

    let args = &fixture.invocations()[0];
    assert!(args.iter().any(|arg| arg == "--lazy-playlist"));
    assert!(args.iter().any(|arg| arg == "--dump-json"));
    assert!(
        args.iter()
            .any(|arg| arg == "ytsearch500:stream first page")
    );

    fixture.release("release-first-page");
    youtui.wait_for_screen("40+ loaded", WAIT_TIMEOUT);

    // Repeating the same completed query is served by the bounded in-memory
    // cache and should not launch another extractor.
    youtui.write_all(b"/stream first page\r");
    youtui.wait_for_screen("Streaming Result 01", WAIT_TIMEOUT);
    assert_eq!(fixture.invocations().len(), 1);

    youtui.quit();
}

#[test]
fn visiting_a_prefetched_page_starts_the_following_page_in_the_background() {
    let fixture = SearchFixture::new(20, true);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"rolling prefetch\r");
    youtui.wait_for_screen("Rolling Result 01", WAIT_TIMEOUT);
    youtui.wait_for_screen("40+ loaded", WAIT_TIMEOUT);

    // The initial extractor supplies both the visible first page and the
    // one-page look-ahead. Remaining on page one must not start another
    // extractor just because that initial request completed.
    let invocations = fixture.invocations();
    assert_eq!(
        invocations.len(),
        1,
        "unexpected invocations: {invocations:?}"
    );
    assert_eq!(playlist_range_start(&invocations[0]), 1);

    // Page two is already cached, so navigation is immediate. Entering it
    // starts a distinct range at item 41, but the fixture holds that process
    // before output to prove the TUI stays on page two while it works.
    youtui.write_all(b"n");
    youtui.wait_for_screen("Rolling Result 21", WAIT_TIMEOUT);
    fixture.wait_for_invocations(2, WAIT_TIMEOUT);
    let invocations = fixture.invocations();
    assert_eq!(
        invocations.len(),
        2,
        "unexpected invocations: {invocations:?}"
    );
    assert_eq!(playlist_range_start(&invocations[1]), 41);
    let screen = youtui.screen();
    assert!(screen.contains("Rolling Result 21"), "{screen}");
    assert!(!screen.contains("Rolling Result 41"), "{screen}");

    fixture.release("release-range-41");
    youtui.wait_for_screen("60+ loaded", WAIT_TIMEOUT);
    let screen = youtui.screen();
    assert!(screen.contains("Rolling Result 21"), "{screen}");
    assert!(!screen.contains("Rolling Result 41"), "{screen}");

    // Page three was populated entirely in the background. Moving to it is
    // instant and starts the same one-page-ahead cycle for page four.
    youtui.write_all(b"n");
    youtui.wait_for_screen("Rolling Result 41", WAIT_TIMEOUT);
    fixture.wait_for_invocations(3, WAIT_TIMEOUT);
    let invocations = fixture.invocations();
    assert_eq!(
        invocations.len(),
        3,
        "unexpected invocations: {invocations:?}"
    );
    assert_eq!(playlist_range_start(&invocations[2]), 61);

    fixture.release("release-range-61");
    youtui.wait_for_screen("80+ loaded", WAIT_TIMEOUT);
    let screen = youtui.screen();
    assert!(screen.contains("Rolling Result 41"), "{screen}");
    assert!(!screen.contains("Rolling Result 61"), "{screen}");

    youtui.quit();
}

#[test]
fn a_new_query_cancels_an_in_flight_search_and_ignores_stale_results() {
    let fixture = SearchFixture::new(1, true);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"blocked first\r");
    wait_for_path(&fixture.control.join("blocked-first-started"), WAIT_TIMEOUT);
    let blocked_pid: i32 = fs::read_to_string(fixture.control.join("blocked-first.pid"))
        .expect("failed to read blocked child PID")
        .parse()
        .expect("invalid blocked child PID");

    // Search runs off the UI thread, so the search field remains usable while
    // the first child is blocked. Starting the replacement must kill/reap it.
    youtui.write_all(b"/replacement query\r");
    youtui.wait_for_screen("Replacement Result", WAIT_TIMEOUT);

    let invocations = fixture.invocations();
    assert_eq!(
        invocations.len(),
        2,
        "unexpected invocations: {invocations:?}"
    );
    assert!(
        invocations[0]
            .iter()
            .any(|arg| arg == "ytsearch500:blocked first")
    );
    assert!(
        invocations[1]
            .iter()
            .any(|arg| arg == "ytsearch500:replacement query")
    );
    wait_for_process_exit(blocked_pid, WAIT_TIMEOUT);
    assert!(!youtui.screen().contains("Stale First Result"));

    youtui.quit();
}

#[test]
fn malformed_lines_are_skipped_without_discarding_adjacent_results() {
    let fixture = SearchFixture::new(2, true);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"malformed output\r");
    youtui.wait_for_screen("Valid Before Malformed", WAIT_TIMEOUT);
    youtui.wait_for_screen("Valid After Malformed", WAIT_TIMEOUT);

    let screen = youtui.screen();
    assert!(!screen.contains("Search failed"), "{screen}");
    youtui.quit();
}

#[test]
fn partial_final_line_does_not_discard_an_already_streamed_result() {
    let fixture = SearchFixture::new(1, true);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"partial final output\r");
    youtui.wait_for_screen("Valid Before Partial EOF", WAIT_TIMEOUT);

    let screen = youtui.screen();
    assert!(!screen.contains("Search failed"), "{screen}");
    youtui.quit();
}

#[test]
fn unknown_duration_is_not_assumed_to_be_a_short() {
    let fixture = SearchFixture::new(1, false);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"unknown duration\r");
    youtui.wait_for_screen("Unknown Duration Result", WAIT_TIMEOUT);

    let screen = youtui.screen();
    assert!(screen.contains("N/A"), "{screen}");
    youtui.quit();
}

#[test]
fn malformed_only_output_is_retryable_and_is_not_cached_as_empty() {
    let fixture = SearchFixture::new(1, true);
    let mut youtui = fixture.spawn_youtui();

    youtui.wait_for_screen("Search", WAIT_TIMEOUT);
    youtui.write_all(b"all malformed\r");
    youtui.wait_for_screen("malformed search entries", WAIT_TIMEOUT);
    assert_eq!(fixture.invocations().len(), 1);

    youtui.write_all(b"/all malformed\r");
    fixture.wait_for_invocations(2, WAIT_TIMEOUT);
    assert_eq!(fixture.invocations().len(), 2);

    youtui.quit();
}

struct SearchFixture {
    _root: tempfile::TempDir,
    _runtime_root: tempfile::TempDir,
    root: PathBuf,
    fake_bin: PathBuf,
    home: PathBuf,
    xdg_config: PathBuf,
    runtime_tmp: PathBuf,
    control: PathBuf,
    invocation_log: PathBuf,
}

impl SearchFixture {
    fn new(results_per_page: usize, include_shorts: bool) -> Self {
        // Keep executable shims off /tmp because hardened Linux systems may
        // mount it noexec. TMPDIR stays short for macOS Unix socket limits.
        let fixture_parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        fs::create_dir_all(&fixture_parent).expect("failed to create fixture parent");
        let root = tempfile::Builder::new()
            .prefix("yts-search-")
            .tempdir_in(fixture_parent)
            .expect("failed to create search fixture");
        let runtime_root = tempfile::Builder::new()
            .prefix("ytr-")
            .tempdir_in("/tmp")
            .expect("failed to create short runtime directory");
        let runtime_tmp = runtime_root.path().to_path_buf();
        let root_path = root.path().to_path_buf();
        let fake_bin = root_path.join("bin");
        let home = root_path.join("home");
        let xdg_config = root_path.join("config");
        let control = root_path.join("control");
        let invocation_log = control.join("invocations.jsonl");

        for directory in [&fake_bin, &home, &xdg_config, &control] {
            fs::create_dir_all(directory).expect("failed to create fixture directory");
        }

        let config = format!(
            "audio_only = true\ninclude_shorts = {include_shorts}\nresults_per_page = {results_per_page}\n"
        );
        write_file(&xdg_config.join("youtui/config.toml"), &config);
        write_file(
            &home.join("Library/Application Support/youtui/config.toml"),
            &config,
        );
        write_executable(&fake_bin.join("yt-dlp"), FAKE_YT_DLP);
        write_executable(&fake_bin.join("mpv"), "#!/bin/sh\nset -eu\nexec sleep 30\n");

        Self {
            _root: root,
            _runtime_root: runtime_root,
            root: root_path,
            fake_bin,
            home,
            xdg_config,
            runtime_tmp,
            control,
            invocation_log,
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
            .env("YOUTUI_SEARCH_CONTROL", &self.control)
            .env("YOUTUI_SEARCH_INVOCATIONS", &self.invocation_log);

        PtyProcess::spawn(command)
    }

    fn release(&self, name: &str) {
        write_file(&self.control.join(name), "release\n");
    }

    fn invocations(&self) -> Vec<Vec<String>> {
        let contents = fs::read_to_string(&self.invocation_log).unwrap_or_default();
        contents
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("invalid invocation log line {line:?}: {error}"))
            })
            .collect()
    }

    fn wait_for_invocations(&self, expected: usize, timeout: Duration) {
        let started = Instant::now();
        while self.invocations().len() < expected {
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for {expected} yt-dlp invocations"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn playlist_range_start(invocation: &[String]) -> usize {
    invocation
        .windows(2)
        .find_map(|args| {
            (args[0] == "--playlist-items").then(|| {
                args[1]
                    .split_once(':')
                    .map_or(args[1].as_str(), |(start, _)| start)
            })
        })
        .expect("yt-dlp invocation omitted --playlist-items")
        .parse()
        .expect("invalid yt-dlp playlist range start")
}

const FAKE_YT_DLP: &str = r#"#!/usr/bin/env python3
import json
import os
import pathlib
import sys
import time

control = pathlib.Path(os.environ["YOUTUI_SEARCH_CONTROL"])
args = sys.argv[1:]
with open(os.environ["YOUTUI_SEARCH_INVOCATIONS"], "a", encoding="utf-8") as log:
    log.write(json.dumps(args) + "\n")
    log.flush()
    os.fsync(log.fileno())

search_arg = next((arg for arg in args if arg.startswith("ytsearch")), "")
query = search_arg.split(":", 1)[1] if ":" in search_arg else ""
playlist_items_index = args.index("--playlist-items")
playlist_start = int(args[playlist_items_index + 1].split(":", 1)[0])

def emit(video_id, title, duration=240, playlist_index=None):
    value = {
        "id": video_id,
        "title": title,
        "duration": duration,
        "duration_string": "4:00",
        "channel": "Search Fixture",
        "view_count": 1234,
    }
    if playlist_index is not None:
        value["playlist_index"] = playlist_index
    print(json.dumps(value), flush=True)

def wait_for(name):
    deadline = time.monotonic() + 60
    while not (control / name).exists():
        if time.monotonic() >= deadline:
            raise SystemExit("timed out waiting for " + name)
        time.sleep(0.01)

if query == "stream first page":
    for index in range(1, 21):
        emit(f"stream-{index}", f"Streaming Result {index:02d}")
    (control / "first-page-emitted").touch()
    wait_for("release-first-page")
    (control / "released").touch()
    for index in range(21, 41):
        emit(f"stream-{index}", f"Streaming Result {index:02d}")
elif query == "rolling prefetch":
    if playlist_start == 1:
        result_count = 40
    else:
        wait_for(f"release-range-{playlist_start}")
        result_count = 20
    for index in range(playlist_start, playlist_start + result_count):
        emit(
            f"rolling-{index}",
            f"Rolling Result {index:02d}",
            playlist_index=index,
        )
elif query == "blocked first":
    (control / "blocked-first.pid").write_text(str(os.getpid()), encoding="utf-8")
    (control / "blocked-first-started").touch()
    wait_for("release-blocked-first")
    emit("stale-first", "Stale First Result")
elif query == "replacement query":
    emit("replacement", "Replacement Result")
elif query == "malformed output":
    emit("valid-before", "Valid Before Malformed")
    sys.stdout.write('{"id":"truncated"')
    sys.stdout.write("\n")
    sys.stdout.flush()
    emit("valid-after", "Valid After Malformed")
elif query == "partial final output":
    emit("valid-before-partial", "Valid Before Partial EOF")
    sys.stdout.write('{"id":"partial-at-eof"')
    sys.stdout.flush()
elif query == "unknown duration":
    print(json.dumps({
        "id": "unknown-duration",
        "title": "Unknown Duration Result",
        "duration_string": None,
        "channel": "Search Fixture",
        "view_count": 5,
    }), flush=True)
elif query == "all malformed":
    print('{not-json', flush=True)
else:
    raise SystemExit("unexpected fixture query: " + query)
"#;

fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("path has no parent"))
        .expect("failed to create parent directory");
    fs::write(path, contents).expect("failed to write fixture file");
}

fn write_executable(path: &Path, contents: &str) {
    write_file(path, contents);
    let mut permissions = fs::metadata(path)
        .expect("failed to stat fake executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to make fixture executable");
}

fn wait_for_path(path: &Path, timeout: Duration) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_process_exit(pid: i32, timeout: Duration) {
    let started = Instant::now();
    loop {
        // SAFETY: signal 0 does not deliver a signal; it only checks whether
        // the exact fixture PID still exists and is visible to this process.
        let result = unsafe { libc::kill(pid, 0) };
        if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        assert!(
            started.elapsed() < timeout,
            "cancelled yt-dlp fixture process {pid} was not reaped"
        );
        thread::sleep(Duration::from_millis(10));
    }
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
        let window_ptr = std::ptr::from_mut(&mut window);
        // SAFETY: openpty initializes both file descriptors on success. Each
        // descriptor is wrapped in exactly one owned File below.
        let result = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                window_ptr,
            )
        };
        assert_eq!(result, 0, "openpty failed: {}", io::Error::last_os_error());

        // SAFETY: openpty returned two valid, uniquely owned descriptors.
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

        let child = command.spawn().expect("failed to launch youtui");
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
        let writer = self.writer.as_mut().expect("PTY writer is closed");
        writer.write_all(input).expect("failed to write PTY input");
        writer.flush().expect("failed to flush PTY input");
    }

    fn screen(&self) -> String {
        let output = self.output.lock().expect("PTY output lock poisoned");
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        parser.process(&output);
        parser.screen().contents()
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

    fn quit(&mut self) {
        self.write_all(b"q");
        let status = self.wait_for_exit(WAIT_TIMEOUT);
        assert!(status.success(), "youtui exited with {status}");
        self.finish_reader();
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
            // SAFETY: process_group is the positive PID of the child launched
            // with process_group(0). The negative PID targets only that group.
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
