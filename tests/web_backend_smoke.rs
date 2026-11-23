use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const VIDEO_A: &str = "abcdefghijk";
const VIDEO_B: &str = "lmnopqrstuv";

#[test]
fn protocol_queue_snapshots_ignore_stale_playback_events() {
    let config_dir = TempDir::new().unwrap();
    let (mut child, mut stdin, mut stdout) = start_backend(config_dir.path(), None);
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    send(&mut stdin, 1, "ping", json!({}));
    assert_eq!(response(&mut stdout, 1)["result"]["version"], 1);

    send(&mut stdin, 2, "queue_add", json!({"track": track(VIDEO_A)}));
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    assert_eq!(
        response(&mut stdout, 2)["result"]["queue"][0]["id"],
        VIDEO_A
    );

    send(
        &mut stdin,
        3,
        "play",
        json!({"track": track(VIDEO_A), "owner": "tab-a"}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    let played = response(&mut stdout, 3)["result"].clone();
    assert_eq!(played["playback_id"], 1);

    send(&mut stdin, 4, "queue_add", json!({"track": track(VIDEO_B)}));
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    response(&mut stdout, 4);

    send(
        &mut stdin,
        5,
        "playback_ended",
        json!({"owner": "tab-a", "playback_id": 0}),
    );
    let stale = response(&mut stdout, 5)["result"].clone();
    assert_eq!(stale["current"]["id"], VIDEO_A);
    assert_eq!(stale["queue"][0]["id"], VIDEO_A);

    send(
        &mut stdin,
        6,
        "playback_ended",
        json!({"owner": "tab-a", "playback_id": 1}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    let first_advance = response(&mut stdout, 6)["result"].clone();
    assert_eq!(first_advance["current"]["id"], VIDEO_A);
    assert_eq!(first_advance["queue"][0]["id"], VIDEO_B);

    send(
        &mut stdin,
        7,
        "playback_ended",
        json!({"owner": "tab-a", "playback_id": 2}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    let second_advance = response(&mut stdout, 7)["result"].clone();
    assert_eq!(second_advance["current"]["id"], VIDEO_B);
    assert_eq!(second_advance["queue"].as_array().unwrap().len(), 0);

    send(&mut stdin, 8, "shutdown", json!({}));
    assert_eq!(response(&mut stdout, 8)["result"]["shutdown"], true);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn eof_cancels_an_active_search_process_tree() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let marker = fake_bin.path().join("child.pid");
    let script = format!(
        "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > '{}'\nwait\n",
        marker.display()
    );
    install_executable(fake_bin.path(), "yt-dlp", &script);
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");
    send(
        &mut stdin,
        1,
        "search",
        json!({"session":"tab-a","query":"blocked","page":1}),
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        marker.exists(),
        "fake yt-dlp did not start its child process"
    );
    let child_pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    drop(stdin);

    let exit_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < exit_deadline,
            "backend did not stop after stdin EOF"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !process_is_running(child_pid),
        "search descendant survived backend EOF"
    );
}

#[test]
fn eof_cancels_an_active_media_process_tree() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let marker = fake_bin.path().join("child.pid");
    let script = format!(
        "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > '{}'\nwait\n",
        marker.display()
    );
    install_executable(fake_bin.path(), "yt-dlp", &script);
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");
    send(&mut stdin, 1, "prepare_media", json!({"video_id": VIDEO_A}));

    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        marker.exists(),
        "fake yt-dlp did not start its child process"
    );
    let child_pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    drop(stdin);

    let exit_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < exit_deadline,
            "backend did not stop after stdin EOF"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !process_is_running(child_pid),
        "media descendant survived backend EOF"
    );
}

#[test]
fn search_progress_and_final_result_echo_generation() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    install_executable(
        fake_bin.path(),
        "yt-dlp",
        "#!/bin/sh\nprintf '%s\\n' '{\"id\":\"abcdefghijk\",\"title\":\"search result\",\"duration_string\":\"1:00\",\"channel\":\"channel\",\"view_count\":42}'\n",
    );
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");
    send(
        &mut stdin,
        12,
        "search",
        json!({"session":"tab-a","query":"test","page":1,"generation":9}),
    );
    let progress = read_frame(&mut stdout);
    assert_eq!(progress["event"], "search_progress");
    assert_eq!(progress["request_id"], 12);
    assert_eq!(progress["result"]["generation"], 9);
    assert_eq!(progress["result"]["results"][0]["id"], VIDEO_A);
    let completed = response(&mut stdout, 12)["result"].clone();
    assert_eq!(completed["generation"], 9);
    assert_eq!(completed["results"][0]["title"], "search result");

    send(&mut stdin, 13, "shutdown", json!({}));
    response(&mut stdout, 13);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn browser_can_start_a_progressive_stream_before_any_media_download() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let downloads = fake_bin.path().join("downloads");
    let script = format!(
        r#"#!/bin/sh
template=""
probe=false
for arg do
    case "$arg" in
        --skip-download) probe=true ;;
        *'%(ext)s') template=$arg ;;
    esac
done
if [ "$probe" = true ]; then
    host="rr1.googlevideo.com"
    case "$arg" in *{VIDEO_B}) host="rr1.googlevideo.com.example.org" ;; esac
    printf '{{"formats":[{{"url":"https://%s/videoplayback?id=test","ext":"mp4","protocol":"https","acodec":"mp4a.40.2","vcodec":"avc1.42001E","height":360}}]}}' "$host" > "${{template%.*}}.info.json"
else
    # The extractor's metadata must be reused for the local fallback.
    case " $* " in *" --load-info-json "*) ;; *) exit 1 ;; esac
    printf x >> '{}'
    printf source > "${{template%.*}}.mp4"
fi
"#,
        downloads.display()
    );
    install_media_tools(fake_bin.path(), &script);
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    send(
        &mut stdin,
        1,
        "prepare_media",
        json!({"video_id": VIDEO_A, "prefer_stream": true}),
    );
    let streamed = response(&mut stdout, 1);
    assert_eq!(
        streamed["result"]["stream_url"],
        "https://rr1.googlevideo.com/videoplayback?id=test"
    );
    assert!(streamed["result"].get("path").is_none());
    assert!(!downloads.exists());

    send(
        &mut stdin,
        2,
        "prepare_media",
        json!({"video_id": VIDEO_B, "prefer_stream": true}),
    );
    let local = response(&mut stdout, 2);
    assert!(Path::new(local["result"]["path"].as_str().unwrap()).is_file());
    assert!(local["result"].get("stream_url").is_none());
    assert_eq!(fs::read(&downloads).unwrap(), b"x");

    send(&mut stdin, 3, "shutdown", json!({}));
    response(&mut stdout, 3);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn media_is_validated_and_cache_files_are_removed_on_shutdown() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let copy_marker = fake_bin.path().join("copied");
    install_executable(
        fake_bin.path(),
        "yt-dlp",
        "#!/bin/sh\nfor arg do case \"$arg\" in *'%(ext)s') out=$(printf '%s' \"$arg\" | sed 's/%(ext)s/mkv/'); printf source > \"$out\";; esac; done\n",
    );
    install_executable(
        fake_bin.path(),
        "ffmpeg",
        &format!(
            "#!/bin/sh\ncopy=false\nfor arg do [ \"$arg\" = \"copy\" ] && copy=true; output=$arg; done\nprintf mp4 > \"$output\"\n[ \"$copy\" = true ] && touch '{}'\n",
            copy_marker.display()
        ),
    );
    install_executable(
        fake_bin.path(),
        "ffprobe",
        "#!/bin/sh\nfor arg do input=$arg; done\ncase \"$input\" in *.mp4) format=mp4;; *) format=matroska;; esac\nprintf '{\"format\":{\"format_name\":\"%s\"},\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"height\":240},{\"codec_type\":\"audio\",\"codec_name\":\"aac\"}]}' \"$format\"\n",
    );
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    let ready = read_frame(&mut stdout);
    assert_eq!(ready["event"], "ready");
    assert!(std::path::Path::new(ready["cache_dir"].as_str().unwrap()).is_absolute());
    send(&mut stdin, 1, "prepare_media", json!({"video_id": VIDEO_A}));
    let prepared = response(&mut stdout, 1)["result"].clone();
    assert_eq!(prepared["mime"], "video/mp4");
    assert_eq!(prepared["media_id"].as_str().unwrap().len(), 17);
    let media_path = std::path::PathBuf::from(prepared["path"].as_str().unwrap());
    assert!(media_path.is_absolute());
    assert!(media_path.is_file());
    assert!(
        copy_marker.exists(),
        "compatible H.264/AAC source was not remuxed"
    );

    send(&mut stdin, 2, "shutdown", json!({}));
    response(&mut stdout, 2);
    drop(stdin);
    assert!(child.wait().unwrap().success());
    assert!(
        !media_path.exists(),
        "managed media cache survived backend shutdown"
    );
}

#[test]
fn completed_media_is_reused_and_missing_cache_files_are_prepared_again() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let downloads = fake_bin.path().join("downloads");
    install_counting_media_tools(fake_bin.path(), &downloads, "");
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    send(&mut stdin, 1, "prepare_media", json!({"video_id": VIDEO_A}));
    let first = response(&mut stdout, 1)["result"].clone();
    send(&mut stdin, 2, "prepare_media", json!({"video_id": VIDEO_A}));
    let reused = response(&mut stdout, 2)["result"].clone();
    assert_eq!(reused["media_id"], first["media_id"]);
    assert_eq!(reused["path"], first["path"]);
    assert_eq!(fs::read(&downloads).unwrap().len(), 1);

    let first_path = std::path::PathBuf::from(first["path"].as_str().unwrap());
    fs::remove_file(&first_path).unwrap();
    send(&mut stdin, 3, "prepare_media", json!({"video_id": VIDEO_A}));
    let rebuilt = response(&mut stdout, 3)["result"].clone();
    assert_ne!(rebuilt["media_id"], first["media_id"]);
    assert_ne!(rebuilt["path"], first["path"]);
    assert!(std::path::Path::new(rebuilt["path"].as_str().unwrap()).is_file());
    assert_eq!(fs::read(&downloads).unwrap().len(), 2);

    send(&mut stdin, 4, "shutdown", json!({}));
    response(&mut stdout, 4);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn failed_media_is_not_cached_and_a_second_request_can_retry() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let downloads = fake_bin.path().join("downloads");
    install_counting_media_tools(
        fake_bin.path(),
        &downloads,
        &format!(
            "if [ ! -e '{}' ]; then touch '{}'; exit 1; fi\n",
            downloads.with_extension("failed").display(),
            downloads.with_extension("failed").display()
        ),
    );
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    send(&mut stdin, 1, "prepare_media", json!({"video_id": VIDEO_A}));
    let failed = read_frame(&mut stdout);
    assert_eq!(failed["id"], 1);
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["error"], "cannot prepare this video");

    send(&mut stdin, 2, "prepare_media", json!({"video_id": VIDEO_A}));
    let prepared = response(&mut stdout, 2)["result"].clone();
    send(&mut stdin, 3, "prepare_media", json!({"video_id": VIDEO_A}));
    let reused = response(&mut stdout, 3)["result"].clone();
    assert_eq!(reused["media_id"], prepared["media_id"]);
    assert_eq!(fs::read(&downloads).unwrap().len(), 2);

    send(&mut stdin, 4, "shutdown", json!({}));
    response(&mut stdout, 4);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn media_preparation_is_single_slot_and_returns_a_wait_message() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let started = fake_bin.path().join("started");
    let yt_dlp = format!(
        "#!/bin/sh\ntouch '{}'\nsleep 0.5\nfor arg do case \"$arg\" in *'%(ext)s') out=$(printf '%s' \"$arg\" | sed 's/%(ext)s/mkv/'); printf source > \"$out\";; esac; done\n",
        started.display()
    );
    install_media_tools(fake_bin.path(), &yt_dlp);
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    send(&mut stdin, 1, "prepare_media", json!({"video_id": VIDEO_A}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(started.exists(), "fake yt-dlp did not start");

    send(&mut stdin, 2, "prepare_media", json!({"video_id": VIDEO_B}));
    let busy = read_frame(&mut stdout);
    assert_eq!(busy["id"], 2);
    assert_eq!(busy["ok"], false);
    assert_eq!(
        busy["error"],
        "a video is already loading; wait for it to finish"
    );
    response(&mut stdout, 1);

    send(&mut stdin, 3, "prepare_media", json!({"video_id": VIDEO_B}));
    response(&mut stdout, 3);

    send(&mut stdin, 4, "shutdown", json!({}));
    response(&mut stdout, 4);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn pruned_cache_entries_are_removed_and_reprepared() {
    let config_dir = TempDir::new().unwrap();
    let fake_bin = TempDir::new().unwrap();
    let downloads = fake_bin.path().join("downloads");
    install_counting_media_tools(fake_bin.path(), &downloads, "");
    let (mut child, mut stdin, mut stdout) =
        start_backend(config_dir.path(), Some(fake_bin.path()));
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    send(&mut stdin, 1, "prepare_media", json!({"video_id": VIDEO_A}));
    let first = response(&mut stdout, 1)["result"].clone();
    let first_path = std::path::PathBuf::from(first["path"].as_str().unwrap());

    for index in 1..=12 {
        let video_id = format!("video{index:06}");
        let request_id = index + 1;
        send(
            &mut stdin,
            request_id,
            "prepare_media",
            json!({"video_id": video_id}),
        );
        response(&mut stdout, request_id);
    }
    assert!(
        !first_path.exists(),
        "the oldest cache file was not evicted"
    );

    send(
        &mut stdin,
        14,
        "prepare_media",
        json!({"video_id": VIDEO_A}),
    );
    let rebuilt = response(&mut stdout, 14)["result"].clone();
    assert_ne!(rebuilt["media_id"], first["media_id"]);
    assert_eq!(fs::read(&downloads).unwrap().len(), 14);

    send(&mut stdin, 15, "shutdown", json!({}));
    response(&mut stdout, 15);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn settings_are_isolated_atomic_and_snapshots_have_revisions() {
    let config_dir = TempDir::new().unwrap();
    let (mut child, mut stdin, mut stdout) = start_backend(config_dir.path(), None);
    let ready = read_frame(&mut stdout);
    assert_eq!(ready["event"], "ready");
    assert!(std::path::Path::new(ready["cache_dir"].as_str().unwrap()).is_absolute());

    send(
        &mut stdin,
        1,
        "settings_update",
        json!({"include_shorts": true, "results_per_page": 0}),
    );
    let invalid = read_frame(&mut stdout);
    assert_eq!(invalid["id"], 1);
    assert_eq!(invalid["ok"], false);
    let config_path = config_dir.path().join("config.toml");
    let original = fs::read_to_string(&config_path).unwrap();
    assert!(original.contains("include_shorts = false"));

    send(
        &mut stdin,
        2,
        "settings_update",
        json!({"results_per_page": 17}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    let updated = response(&mut stdout, 2)["result"].clone();
    assert_eq!(updated["revision"], 1);
    assert_eq!(updated["settings"]["results_per_page"], 17);
    assert!(
        fs::read_to_string(&config_path)
            .unwrap()
            .contains("results_per_page = 17")
    );

    send(&mut stdin, 3, "queue_add", json!({"track": track(VIDEO_B)}));
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    response(&mut stdout, 3);
    send(
        &mut stdin,
        4,
        "settings_update",
        json!({"auto_play_queue": false}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    response(&mut stdout, 4);
    send(
        &mut stdin,
        5,
        "play",
        json!({"track": track(VIDEO_A), "owner": "tab-a"}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    let playback = response(&mut stdout, 5)["result"].clone();
    let playback_id = playback["playback_id"].as_u64().unwrap();
    send(
        &mut stdin,
        6,
        "playback_ended",
        json!({"owner": "tab-a", "playback_id": playback_id}),
    );
    assert_eq!(read_frame(&mut stdout)["event"], "state_changed");
    let ended = response(&mut stdout, 6)["result"].clone();
    assert!(ended["current"].is_null());
    assert_eq!(ended["queue"][0]["id"], VIDEO_B);

    send(
        &mut stdin,
        7,
        "playback_ended",
        json!({"owner": "tab-a", "playback_id": playback_id}),
    );
    let repeated = response(&mut stdout, 7)["result"].clone();
    assert!(repeated["current"].is_null());
    assert_eq!(repeated["queue"][0]["id"], VIDEO_B);
    assert_eq!(repeated["revision"], ended["revision"]);

    send(&mut stdin, 8, "shutdown", json!({}));
    response(&mut stdout, 8);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn queue_mutations_reject_stale_revisions_without_changing_state() {
    let config_dir = TempDir::new().unwrap();
    let (mut child, mut stdin, mut stdout) = start_backend(config_dir.path(), None);
    assert_eq!(read_frame(&mut stdout)["event"], "ready");

    for (id, video) in [(1, VIDEO_A), (2, VIDEO_B)] {
        send(&mut stdin, id, "queue_add", json!({"track": track(video)}));
        response(&mut stdout, id);
    }
    for (id, command) in [(3, "queue_play"), (4, "queue_remove")] {
        send(
            &mut stdin,
            id,
            command,
            json!({"index": 0, "owner": "tab-a", "expected_revision": 1}),
        );
        let rejected = read_frame(&mut stdout);
        assert_eq!(rejected["id"], id);
        assert_eq!(rejected["ok"], false);
        assert!(
            rejected["error"]
                .as_str()
                .unwrap()
                .contains("queue changed")
        );
    }
    send(&mut stdin, 5, "state", json!({}));
    let unchanged = response(&mut stdout, 5)["result"].clone();
    assert_eq!(unchanged["revision"], 2);
    assert_eq!(unchanged["queue"].as_array().unwrap().len(), 2);
    assert!(unchanged["current"].is_null());

    send(
        &mut stdin,
        6,
        "queue_play",
        json!({"index": 1, "owner": "tab-a", "expected_revision": 2}),
    );
    let played = response(&mut stdout, 6)["result"].clone();
    assert_eq!(played["revision"], 3);
    assert_eq!(played["current"]["id"], VIDEO_B);
    assert_eq!(played["queue"][0]["id"], VIDEO_A);
    send(&mut stdin, 7, "shutdown", json!({}));
    response(&mut stdout, 7);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn oversized_metadata_cannot_poison_queue_snapshots() {
    let config_dir = TempDir::new().unwrap();
    let (mut child, mut stdin, mut stdout) = start_backend(config_dir.path(), None);
    read_frame(&mut stdout);
    let mut oversized = track(VIDEO_A);
    oversized["title"] = Value::String("x".repeat(8192));
    send(&mut stdin, 1, "queue_add", json!({"track": oversized}));
    let rejected = read_frame(&mut stdout);
    assert_eq!(rejected["ok"], false);
    assert!(rejected["error"].as_str().unwrap().contains("metadata"));
    send(&mut stdin, 2, "state", json!({}));
    let unchanged = response(&mut stdout, 2)["result"].clone();
    assert_eq!(unchanged["revision"], 0);
    assert_eq!(unchanged["queue"], json!([]));
    send(&mut stdin, 3, "shutdown", json!({}));
    response(&mut stdout, 3);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn a_full_queue_rejects_new_entries_and_remains_readable() {
    let config_dir = TempDir::new().unwrap();
    let (mut child, mut stdin, mut stdout) = start_backend(config_dir.path(), None);
    read_frame(&mut stdout);
    for id in 1..=100 {
        send(
            &mut stdin,
            id,
            "queue_add",
            json!({"track": track(VIDEO_A)}),
        );
        response(&mut stdout, id);
    }
    send(
        &mut stdin,
        101,
        "queue_add",
        json!({"track": track(VIDEO_B)}),
    );
    let rejected = read_frame(&mut stdout);
    assert_eq!(rejected["ok"], false);
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("queue is full")
    );
    send(&mut stdin, 102, "state", json!({}));
    let unchanged = response(&mut stdout, 102)["result"].clone();
    assert_eq!(unchanged["revision"], 100);
    assert_eq!(unchanged["queue"].as_array().unwrap().len(), 100);
    send(&mut stdin, 103, "shutdown", json!({}));
    response(&mut stdout, 103);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

fn start_backend(
    config_path: &Path,
    extra_path: Option<&Path>,
) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let binary = env!("CARGO_BIN_EXE_youtui");
    let mut command = Command::new(binary);
    command
        .arg("--backend-stdio")
        .env("YOUTUI_CONFIG_PATH", config_path.join("config.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(path) = extra_path {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(path.to_path_buf())
            .chain(std::env::split_paths(&current))
            .collect::<Vec<_>>();
        command.env("PATH", std::env::join_paths(paths).unwrap());
    }
    let mut child = command.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdin, stdout)
}

fn send(stdin: &mut ChildStdin, id: u64, command: &str, params: Value) {
    let bytes = serde_json::to_vec(&json!({"version":1,"id":id,"command":command,"params":params}))
        .unwrap();
    stdin
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stdin.write_all(&bytes).unwrap();
    stdin.flush().unwrap();
}

fn read_frame(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut length = [0_u8; 4];
    stdout.read_exact(&mut length).unwrap();
    let mut bytes = vec![0_u8; u32::from_be_bytes(length) as usize];
    stdout.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn response(stdout: &mut BufReader<ChildStdout>, id: u64) -> Value {
    loop {
        let frame = read_frame(stdout);
        if frame.get("id").and_then(Value::as_u64) == Some(id) {
            assert_eq!(frame["ok"], true, "backend returned {frame}");
            return frame;
        }
    }
}

fn track(id: &str) -> Value {
    json!({"id":id,"title":"title","duration":"1:00","channel":"channel","views":"5","published":"today"})
}

fn install_counting_media_tools(directory: &Path, downloads: &Path, before_download: &str) {
    let yt_dlp = format!(
        "#!/bin/sh\nprintf x >> '{}'\n{}for arg do case \"$arg\" in *'%(ext)s') out=$(printf '%s' \"$arg\" | sed 's/%(ext)s/mkv/'); printf source > \"$out\";; esac; done\n",
        downloads.display(),
        before_download
    );
    install_media_tools(directory, &yt_dlp);
}

fn install_media_tools(directory: &Path, yt_dlp: &str) {
    install_executable(directory, "yt-dlp", yt_dlp);
    install_executable(
        directory,
        "ffmpeg",
        "#!/bin/sh\nfor arg do output=$arg; done\nprintf mp4 > \"$output\"\n",
    );
    install_executable(
        directory,
        "ffprobe",
        "#!/bin/sh\nfor arg do input=$arg; done\ncase \"$input\" in *.mp4) format=mp4;; *) format=matroska;; esac\nprintf '{\"format\":{\"format_name\":\"%s\"},\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"height\":240},{\"codec_type\":\"audio\",\"codec_name\":\"aac\"}]}' \"$format\"\n",
    );
}

fn install_executable(directory: &Path, name: &str, contents: &str) {
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(unix)]
fn process_is_running(pid: i32) -> bool {
    // `/bin/kill -0` reports whether the child process still exists.
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(unix))]
fn process_is_running(_pid: i32) -> bool {
    false
}
