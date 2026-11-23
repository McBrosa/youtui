//! JSON framed backend runtime used by the local Phoenix interface.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::config::{Config, MAX_RESULTS_PER_PAGE, MIN_RESULTS_PER_PAGE};
use crate::queue::Queue;
use crate::search::{PaginatedSearch, SearchResult};

const PROTOCOL_VERSION: u64 = 1;
const MAX_FRAME_SIZE: usize = 1024 * 1024;
const MAX_QUEUE_TRACKS: usize = 100;
const MAX_TRACK_BYTES: usize = 8192;
const MAX_SESSION_LENGTH: usize = 128;
const SEARCH_ID_LENGTH: usize = 11;
const MEDIA_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MEDIA_CACHE_LIMIT: usize = 12;
const MEDIA_JOB_LIMIT: usize = 1;
const MAX_SEARCH_SESSIONS: usize = 32;
const MAX_ACTIVE_ASYNC_JOBS: usize = 8;
const MEDIA_FILE_LIMIT: u64 = 512 * 1024 * 1024;
const MEDIA_CACHE_BYTE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone)]
struct CachedMedia {
    media_id: String,
    path: PathBuf,
}

#[derive(Default)]
struct MediaCache {
    by_video_id: HashMap<String, CachedMedia>,
}

/// Runs the version 1 framed protocol until stdin closes or shutdown is requested.
pub fn run_stdio() -> Result<()> {
    crate::cleanup::INTERRUPTED.store(false, Ordering::SeqCst);
    crate::cleanup::setup_signal_handler();
    let cache_dir = tempfile::Builder::new()
        .prefix("youtui-web-")
        .tempdir()
        .context("cannot create web media cache")?;
    let config = Config::load_or_create().context("cannot load settings")?;
    let mut state = BackendState::new(config);
    let output = Arc::new(Mutex::new(BufWriter::new(io::stdout())));
    let requests = Arc::new(Mutex::new(HashSet::new()));
    let media_slots = Arc::new(Mutex::new(0_usize));
    let media_cache = Arc::new(Mutex::new(MediaCache::default()));
    let media_counter = Arc::new(AtomicU64::new(1));
    let cache_path = Arc::new(cache_dir.path().to_path_buf());
    let shutting_down = Arc::new(AtomicBool::new(false));
    let mut jobs = Vec::<JoinHandle<()>>::new();

    let canonical_cache_path = cache_dir
        .path()
        .canonicalize()
        .context("cannot resolve web media cache")?;
    write_event(
        &output,
        json!({
            "version": PROTOCOL_VERSION,
            "event": "ready",
            "cache_dir": canonical_cache_path,
        }),
    )?;

    let (frame_sender, frame_receiver) = mpsc::sync_channel::<Result<Option<Vec<u8>>>>(16);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            match read_frame(&mut reader) {
                Ok(frame) => {
                    let done = frame.is_none();
                    if frame_sender.send(Ok(frame)).is_err() || done {
                        break;
                    }
                }
                Err(error) => {
                    let _ = frame_sender.send(Err(error));
                    break;
                }
            }
        }
    });
    let run_result = (|| -> Result<()> {
        loop {
            if crate::cleanup::INTERRUPTED.load(Ordering::SeqCst) {
                break;
            }
            let frame = match frame_receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(Some(frame))) => frame,
                Ok(Ok(None)) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Err(error)) => {
                    return Err(error);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            };
            jobs.retain(|job| !job.is_finished());
            let request = match parse_request(&frame) {
                Ok(request) => request,
                Err((id, message)) => {
                    let response = error_response(id, message);
                    write_frame(&output, &response)?;
                    continue;
                }
            };

            if !reserve_request(&requests, request.id) {
                write_frame(
                    &output,
                    &error_response(Some(request.id), "request id is already active"),
                )?;
                continue;
            }

            match request.command.as_str() {
                "ping" => finish_request(
                    &output,
                    &requests,
                    request.id,
                    Ok(json!({"version": PROTOCOL_VERSION})),
                )?,
                "state" => {
                    finish_request(&output, &requests, request.id, Ok(state.snapshot()))?;
                }
                "search" => {
                    let id = request.id;
                    if jobs.iter().filter(|job| !job.is_finished()).count() >= MAX_ACTIVE_ASYNC_JOBS
                    {
                        finish_request(
                            &output,
                            &requests,
                            id,
                            Err(anyhow!("too many backend jobs are active")),
                        )?;
                        continue;
                    }
                    match prepare_search(&request.params, id, &mut state) {
                        Ok(search) => {
                            let output = Arc::clone(&output);
                            let requests = Arc::clone(&requests);
                            jobs.push(thread::spawn(move || {
                                run_search(search, output, requests);
                            }));
                        }
                        Err(error) => finish_request(&output, &requests, id, Err(error))?,
                    }
                }
                "cancel_search" => {
                    let result = cancel_search(&request.params, &state);
                    finish_request(&output, &requests, request.id, result)?;
                }
                "prepare_media" => {
                    let id = request.id;
                    let video_id = match required_video_id(&request.params) {
                        Ok(video_id) => video_id,
                        Err(error) => {
                            finish_request(&output, &requests, id, Err(error))?;
                            continue;
                        }
                    };
                    match cached_media(&video_id, &cache_path, &media_cache) {
                        Ok(Some(result)) => {
                            finish_request(&output, &requests, id, Ok(result))?;
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            finish_request(&output, &requests, id, Err(error))?;
                            continue;
                        }
                    }
                    if !reserve_media_slot(&media_slots) {
                        finish_request(
                            &output,
                            &requests,
                            id,
                            Err(anyhow!(
                                "media preparation is busy; wait for the current video"
                            )),
                        )?;
                        continue;
                    }
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let output = Arc::clone(&output);
                    let requests = Arc::clone(&requests);
                    let slots = Arc::clone(&media_slots);
                    let cache_path = Arc::clone(&cache_path);
                    let media_cache = Arc::clone(&media_cache);
                    let counter = Arc::clone(&media_counter);
                    let shutdown = Arc::clone(&shutting_down);
                    jobs.push(thread::spawn(move || {
                        let result = prepare_or_reuse_media(
                            &video_id,
                            &cache_path,
                            &media_cache,
                            &counter,
                            &cancelled,
                            &shutdown,
                        );
                        release_media_slot(&slots);
                        let _ = finish_request(&output, &requests, id, result);
                    }));
                }
                "shutdown" => {
                    if !request.params.is_empty() {
                        finish_request(
                            &output,
                            &requests,
                            request.id,
                            Err(anyhow!("shutdown takes no parameters")),
                        )?;
                        continue;
                    }
                    finish_request(
                        &output,
                        &requests,
                        request.id,
                        Ok(json!({"shutdown": true})),
                    )?;
                    break;
                }
                command => {
                    let result = run_state_command(command, &request.params, &mut state, &output);
                    finish_request(&output, &requests, request.id, result)?;
                }
            }
        }
        Ok(())
    })();

    shutting_down.store(true, Ordering::SeqCst);
    for entry in &state.searches {
        if let Some(cancelled) = entry.cancelled.upgrade() {
            cancelled.store(true, Ordering::SeqCst);
        }
    }
    for job in jobs {
        let _ = job.join();
    }
    run_result?;
    Ok(())
}

fn run_state_command(
    command: &str,
    params: &Map<String, Value>,
    state: &mut BackendState,
    output: &Arc<Mutex<BufWriter<io::Stdout>>>,
) -> Result<Value> {
    let changed = match command {
        "queue_add" => {
            let track = parse_track(params.get("track"))?;
            if state.queue.len() >= MAX_QUEUE_TRACKS {
                bail!("the web queue is full (100 upcoming videos)");
            }
            state.queue.push_back(track);
            true
        }
        "queue_remove" => {
            let index = required_index(params, "index")?;
            ensure_expected_revision(params, state.revision)?;
            ensure_known(state.queue.remove(index), "queue index is out of range")?;
            true
        }
        "queue_clear" => {
            state.queue.clear();
            true
        }
        "queue_play" => {
            let index = required_index(params, "index")?;
            let owner = required_owner(params)?;
            ensure_expected_revision(params, state.revision)?;
            let track = ensure_known(state.queue.remove(index), "queue index is out of range")?;
            state.start_playback(track, owner);
            true
        }
        "play" => {
            let track = parse_track(params.get("track"))?;
            let owner = required_owner(params)?;
            state.start_playback(track, owner);
            true
        }
        "skip" | "playback_ended" => {
            let owner = required_owner(params)?;
            let playback_id = required_u64(params, "playback_id")?;
            let matches_current = state.current.is_some()
                && state.owner.as_deref() == Some(owner.as_str())
                && state.playback_id == playback_id;
            if matches_current && (command == "skip" || state.config.auto_play_queue) {
                state.advance(owner);
                true
            } else if matches_current {
                state.current = None;
                state.owner = None;
                state.playback_id = state.playback_id.saturating_add(1);
                true
            } else {
                false
            }
        }
        "settings_update" => {
            let mut updated = state.config.clone();
            update_settings(&mut updated, params)?;
            updated.save().context("cannot save settings")?;
            state.config = updated;
            true
        }
        _ => bail!("unknown command"),
    };
    if changed {
        state.revision = state.revision.saturating_add(1);
    }
    let snapshot = state.snapshot();
    if changed {
        write_event(
            output,
            json!({"version": PROTOCOL_VERSION, "event": "state_changed", "result": snapshot}),
        )?;
    }
    Ok(snapshot)
}

fn prepare_search(
    params: &Map<String, Value>,
    request_id: u64,
    state: &mut BackendState,
) -> Result<SearchJob> {
    let session = required_session(params)?;
    let query = required_string(params, "query", 512)?;
    if query.trim().is_empty() {
        bail!("query cannot be empty");
    }
    let page = required_u64(params, "page")?;
    if page == 0 || page > 1_000_000 {
        bail!("page must be between 1 and 1000000");
    }
    let generation = match params.get("generation") {
        None => 0,
        Some(value) => value
            .as_u64()
            .filter(|generation| *generation > 0)
            .ok_or_else(|| anyhow!("generation must be a positive unsigned integer"))?,
    };

    let page = usize::try_from(page).context("page is out of range")?;
    let previous = state
        .searches
        .iter()
        .position(|entry| entry.session == session)
        .and_then(|index| state.searches.remove(index));
    if let Some(entry) = &previous {
        if let Some(cancelled) = entry.cancelled.upgrade() {
            cancelled.store(true, Ordering::SeqCst);
        }
    } else if state.searches.len() >= MAX_SEARCH_SESSIONS {
        let idle = state
            .searches
            .iter()
            .position(|entry| entry.cancelled.strong_count() == 0)
            .ok_or_else(|| anyhow!("too many search sessions are active"))?;
        state.searches.remove(idle);
    }

    let page_size = state.config.results_per_page;
    let filter_shorts = !state.config.include_shorts;
    let sort_by_date = state.config.sort_by_date;
    let search = match previous {
        Some(entry)
            if entry.query == query
                && entry.page_size == page_size
                && entry.filter_shorts == filter_shorts
                && entry.sort_by_date == sort_by_date =>
        {
            entry.search
        }
        _ => Arc::new(Mutex::new(PaginatedSearch::new(
            &query,
            page_size,
            filter_shorts,
        ))),
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    state.searches.push_back(SearchCacheEntry {
        session: session.clone(),
        query: query.clone(),
        page_size,
        filter_shorts,
        sort_by_date,
        cancelled: Arc::downgrade(&cancelled),
        search: Arc::clone(&search),
    });

    Ok(SearchJob {
        session,
        query,
        page,
        request_id,
        generation,
        sort_by_date,
        cancelled,
        search,
    })
}

fn run_search(
    job: SearchJob,
    output: Arc<Mutex<BufWriter<io::Stdout>>>,
    requests: Arc<Mutex<HashSet<u64>>>,
) {
    let result = (|| -> Result<Value> {
        let mut search = job
            .search
            .lock()
            .map_err(|_| anyhow!("search state is unavailable"))?;
        search.sort_by_date = job.sort_by_date;
        search.ensure_page_with_cancel_and_progress(job.page - 1, &job.cancelled, |progress| {
            let snapshot =
                search_snapshot(progress, &job.session, &job.query, job.page, job.generation);
            let event = json!({
                "version": PROTOCOL_VERSION,
                "event": "search_progress",
                "request_id": job.request_id,
                "result": snapshot,
            });
            let _ = write_event(&output, event);
        })?;
        Ok(search_snapshot(
            &search,
            &job.session,
            &job.query,
            job.page,
            job.generation,
        ))
    })();
    let _ = finish_request(&output, &requests, job.request_id, result);
}

fn cancel_search(params: &Map<String, Value>, state: &BackendState) -> Result<Value> {
    let session = required_session(params)?;
    let cancelled = state
        .searches
        .iter()
        .find(|entry| entry.session == session)
        .and_then(|entry| entry.cancelled.upgrade())
        .is_some_and(|token| !token.swap(true, Ordering::SeqCst));
    Ok(json!({"cancelled": cancelled}))
}

fn cached_media(
    video_id: &str,
    cache_dir: &Path,
    media_cache: &Arc<Mutex<MediaCache>>,
) -> Result<Option<Value>> {
    let cache_root = cache_dir
        .canonicalize()
        .context("cannot resolve media cache root")?;
    let mut media_cache = media_cache
        .lock()
        .map_err(|_| anyhow!("media cache is unavailable"))?;
    let Some(entry) = media_cache.by_video_id.get(video_id).cloned() else {
        return Ok(None);
    };

    match cached_media_path(&cache_root, &entry) {
        Ok(path) => Ok(Some(json!({
            "media_id": entry.media_id,
            "path": path,
            "mime": "video/mp4",
        }))),
        Err(_) => {
            media_cache.by_video_id.remove(video_id);
            Ok(None)
        }
    }
}

fn cached_media_path(cache_root: &Path, entry: &CachedMedia) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(&entry.path).context("cached media is unavailable")?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MEDIA_FILE_LIMIT {
        bail!("cached media file is invalid");
    }
    let path = entry
        .path
        .canonicalize()
        .context("cannot resolve cached media path")?;
    let expected_name = format!("{}.mp4", entry.media_id);
    if !path.starts_with(cache_root)
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
    {
        bail!("cached media path is outside the managed cache");
    }
    Ok(path)
}

fn prepare_or_reuse_media(
    video_id: &str,
    cache_dir: &Path,
    media_cache: &Arc<Mutex<MediaCache>>,
    counter: &AtomicU64,
    cancelled: &AtomicBool,
    shutting_down: &AtomicBool,
) -> Result<Value> {
    if let Some(result) = cached_media(video_id, cache_dir, media_cache)? {
        return Ok(result);
    }

    prepare_media(
        video_id,
        cache_dir,
        media_cache,
        counter,
        cancelled,
        shutting_down,
    )
}

fn prepare_media(
    video_id: &str,
    cache_dir: &Path,
    media_cache: &Arc<Mutex<MediaCache>>,
    counter: &AtomicU64,
    cancelled: &AtomicBool,
    shutting_down: &AtomicBool,
) -> Result<Value> {
    validate_video_id(video_id)?;
    let job_dir = tempfile::Builder::new()
        .prefix("prepare-")
        .tempdir_in(cache_dir)
        .context("cannot create media job directory")?;
    let source_prefix = job_dir.path().join("source");
    let source_template = format!("{}.%(ext)s", source_prefix.display());
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let format = "bestvideo[height<=720][vcodec^=avc1]+bestaudio[acodec^=mp4a]/best[height<=720][vcodec^=avc1][acodec^=mp4a]/bestvideo[height<=720]+bestaudio/best[height<=720]";
    run_checked_process(
        Command::new("yt-dlp")
            .args([
                "--no-playlist",
                "--no-warnings",
                "--max-filesize",
                "512M",
                "--format",
                format,
                "--merge-output-format",
                "mkv",
                "--output",
            ])
            .arg(&source_template)
            .arg(&url),
        MEDIA_TIMEOUT,
        cancelled,
        shutting_down,
    )
    .context("cannot download media")?;

    ensure_active(cancelled, shutting_down)?;
    let source = fs::read_dir(job_dir.path())?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("source.") && !name.contains(".part"))
                && matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("mp4" | "mkv" | "webm" | "m4v")
                )
        })
        .max_by_key(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(0)
        })
        .ok_or_else(|| anyhow!("yt-dlp did not create a media file"))?;
    if fs::metadata(&source)?.len() > MEDIA_FILE_LIMIT {
        bail!("source media exceeds the size limit");
    }
    let source_probe = probe_media(&source, cancelled, shutting_down)?;
    let copy_compatible = has_codec(&source_probe, "video", "h264")
        && has_codec(&source_probe, "audio", "aac")
        && video_height(&source_probe).is_some_and(|height| height <= 720);
    let prepared = job_dir.path().join("prepared.mp4");
    let mut conversion = Command::new("ffmpeg");
    conversion
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&source)
        .args(["-map", "0:v:0", "-map", "0:a:0"]);
    if copy_compatible {
        conversion.args(["-c", "copy", "-movflags", "+faststart"]);
    } else {
        conversion.args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-vf",
            "scale=-2:min(720\\,ih)",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
        ]);
    }
    conversion.args(["-fs", "536870912"]).arg(&prepared);
    run_checked_process(&mut conversion, MEDIA_TIMEOUT, cancelled, shutting_down)
        .context("cannot convert media to MP4")?;
    if fs::metadata(&prepared)?.len() > MEDIA_FILE_LIMIT {
        bail!("prepared media exceeds the size limit");
    }
    let prepared_probe = validate_mp4(&prepared, cancelled, shutting_down)?;
    if let (Some(source_duration), Some(prepared_duration)) = (
        media_duration(&source_probe),
        media_duration(&prepared_probe),
    ) && output_is_truncated(source_duration, prepared_duration)
    {
        bail!("prepared media is shorter than the source");
    }

    let media_number = counter.fetch_add(1, Ordering::Relaxed);
    let media_id = format!("m{media_number:016x}");
    let destination = cache_dir.join(format!("{media_id}.mp4"));
    let mut media_cache = media_cache
        .lock()
        .map_err(|_| anyhow!("media cache is unavailable"))?;
    fs::rename(&prepared, &destination).context("cannot publish prepared media")?;
    if let Err(error) = prune_media_cache(cache_dir, &destination) {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    let cache_root = cache_dir
        .canonicalize()
        .context("cannot resolve media cache root")?;
    let path = destination
        .canonicalize()
        .context("cannot resolve prepared media path")?;
    media_cache
        .by_video_id
        .retain(|_, entry| cached_media_path(&cache_root, entry).is_ok());
    media_cache.by_video_id.insert(
        video_id.to_string(),
        CachedMedia {
            media_id: media_id.clone(),
            path: path.clone(),
        },
    );
    Ok(json!({"media_id": media_id, "path": path, "mime": "video/mp4"}))
}

fn validate_mp4(path: &Path, cancelled: &AtomicBool, shutting_down: &AtomicBool) -> Result<Value> {
    let probe = probe_media(path, cancelled, shutting_down)?;
    let format_is_mp4 = probe
        .pointer("/format/format_name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.contains("mp4"));
    if !has_codec(&probe, "video", "h264")
        || !has_codec(&probe, "audio", "aac")
        || !format_is_mp4
        || !path.is_file()
        || fs::metadata(path)?.len() == 0
    {
        bail!("prepared media is not a playable H.264/AAC MP4");
    }
    Ok(probe)
}

fn probe_media(path: &Path, cancelled: &AtomicBool, shutting_down: &AtomicBool) -> Result<Value> {
    let output = run_capture_process(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=format_name,duration:stream=codec_type,codec_name,height",
                "-of",
                "json",
            ])
            .arg(path),
        Duration::from_secs(30),
        cancelled,
        shutting_down,
    )
    .context("cannot inspect prepared media")?;
    serde_json::from_slice(&output).context("ffprobe returned invalid output")
}

fn media_duration(probe: &Value) -> Option<f64> {
    probe
        .pointer("/format/duration")
        .and_then(Value::as_str)
        .and_then(|duration| duration.parse::<f64>().ok())
}

fn output_is_truncated(source_duration: f64, output_duration: f64) -> bool {
    source_duration > 1.0 && output_duration + (source_duration * 0.02).max(1.0) < source_duration
}

fn has_codec(probe: &Value, kind: &str, codec: &str) -> bool {
    probe
        .get("streams")
        .and_then(Value::as_array)
        .is_some_and(|streams| {
            streams.iter().any(|stream| {
                stream.get("codec_type").and_then(Value::as_str) == Some(kind)
                    && stream.get("codec_name").and_then(Value::as_str) == Some(codec)
            })
        })
}

fn video_height(probe: &Value) -> Option<u64> {
    probe
        .get("streams")
        .and_then(Value::as_array)?
        .iter()
        .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))?
        .get("height")
        .and_then(Value::as_u64)
}

fn run_checked_process(
    command: &mut Command,
    timeout: Duration,
    cancelled: &AtomicBool,
    shutting_down: &AtomicBool,
) -> Result<()> {
    let mut child = spawn_process(command, Stdio::null())?;
    match wait_for_process(&mut child, timeout, cancelled, shutting_down)? {
        ProcessEnd::Success => Ok(()),
        ProcessEnd::Failed(code) => bail!("media command failed with exit code {code}"),
        ProcessEnd::Cancelled => bail!("media preparation cancelled"),
        ProcessEnd::TimedOut => bail!("media preparation timed out"),
    }
}

fn run_capture_process(
    command: &mut Command,
    timeout: Duration,
    cancelled: &AtomicBool,
    shutting_down: &AtomicBool,
) -> Result<Vec<u8>> {
    let mut child = spawn_process(command, Stdio::piped())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("media command has no output pipe"))?;
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let end = wait_for_process(&mut child, timeout, cancelled, shutting_down);
    match &end {
        Ok(_) => kill_process_group(&child),
        Err(_) => terminate_process_tree(&mut child),
    }
    let output = reader
        .join()
        .map_err(|_| anyhow!("media output reader stopped unexpectedly"))??;
    let end = end?;
    match end {
        ProcessEnd::Success => Ok(output),
        ProcessEnd::Failed(code) => bail!("media inspection failed with exit code {code}"),
        ProcessEnd::Cancelled => bail!("media preparation cancelled"),
        ProcessEnd::TimedOut => bail!("media inspection timed out"),
    }
}

fn spawn_process(command: &mut Command, stdout: Stdio) -> Result<Child> {
    command.stdout(stdout).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid only creates a process group before exec; it does not access memory.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command.spawn().context("cannot start media command")
}

fn wait_for_process(
    child: &mut Child,
    timeout: Duration,
    cancelled: &AtomicBool,
    shutting_down: &AtomicBool,
) -> Result<ProcessEnd> {
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::SeqCst) || shutting_down.load(Ordering::SeqCst) {
            terminate_process_tree(child);
            return Ok(ProcessEnd::Cancelled);
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(child);
            return Ok(ProcessEnd::TimedOut);
        }
        if let Some(status) = child.try_wait().context("cannot monitor media command")? {
            return Ok(match status.code() {
                Some(0) => ProcessEnd::Success,
                Some(code) => ProcessEnd::Failed(code),
                None => ProcessEnd::Failed(-1),
            });
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

fn terminate_process_tree(child: &mut Child) {
    kill_process_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

fn kill_process_group(child: &Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: the child was started in its own process group by spawn_process.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

fn prune_media_cache(cache_dir: &Path, current: &Path) -> Result<()> {
    let mut entries = fs::read_dir(cache_dir)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("mp4"))
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            Some((path, metadata.modified().ok()?, metadata.len()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|(_, modified, _)| *modified);
    let mut total_bytes = entries.iter().map(|(_, _, size)| *size).sum::<u64>();
    while entries.len() > MEDIA_CACHE_LIMIT || total_bytes > MEDIA_CACHE_BYTE_LIMIT {
        let Some(index) = entries.iter().position(|(path, _, _)| path != current) else {
            bail!("media cache cannot fit the prepared file");
        };
        let (path, _, size) = entries.remove(index);
        fs::remove_file(path).context("cannot evict old prepared media")?;
        total_bytes = total_bytes.saturating_sub(size);
    }
    Ok(())
}

fn update_settings(config: &mut Config, params: &Map<String, Value>) -> Result<()> {
    for (key, value) in params {
        match key.as_str() {
            "include_shorts" => config.include_shorts = required_bool_value(value, key)?,
            "sort_by_date" => config.sort_by_date = required_bool_value(value, key)?,
            "auto_play_queue" => config.auto_play_queue = required_bool_value(value, key)?,
            "results_per_page" => {
                let value = value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| anyhow!("results_per_page must be an unsigned integer"))?;
                if !(MIN_RESULTS_PER_PAGE..=MAX_RESULTS_PER_PAGE).contains(&value) {
                    bail!(
                        "results_per_page must be between {MIN_RESULTS_PER_PAGE} and {MAX_RESULTS_PER_PAGE}"
                    );
                }
                config.results_per_page = value;
            }
            _ => bail!("unsupported setting"),
        }
    }
    Ok(())
}

fn parse_track(value: Option<&Value>) -> Result<SearchResult> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("track must be an object"))?;
    let track = SearchResult {
        id: required_track_field(object, "id")?,
        title: required_track_field(object, "title")?,
        duration: required_track_field(object, "duration")?,
        channel: required_track_field(object, "channel")?,
        views: required_track_field(object, "views")?,
        published: required_track_field(object, "published")?,
    };
    validate_video_id(&track.id)?;
    if serde_json::to_vec(&track_value(&track))?.len() > MAX_TRACK_BYTES {
        bail!("track metadata exceeds the 8 KiB limit");
    }
    Ok(track)
}

fn ensure_expected_revision(params: &Map<String, Value>, revision: u64) -> Result<()> {
    if params.contains_key("expected_revision")
        && required_u64(params, "expected_revision")? != revision
    {
        bail!("the queue changed; select the item again");
    }
    Ok(())
}

fn required_track_field(object: &Map<String, Value>, key: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("track field {key} must be a string"))
}

fn validate_video_id(video_id: &str) -> Result<()> {
    if video_id.len() != SEARCH_ID_LENGTH
        || !video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("video_id must be a valid YouTube video ID");
    }
    Ok(())
}

fn required_video_id(params: &Map<String, Value>) -> Result<String> {
    let video_id = required_string(params, "video_id", SEARCH_ID_LENGTH)?;
    validate_video_id(&video_id)?;
    Ok(video_id)
}

fn required_session(params: &Map<String, Value>) -> Result<String> {
    required_string(params, "session", MAX_SESSION_LENGTH)
}

fn required_owner(params: &Map<String, Value>) -> Result<String> {
    let owner = required_string(params, "owner", MAX_SESSION_LENGTH)?;
    if owner.trim().is_empty() {
        bail!("owner cannot be empty");
    }
    Ok(owner)
}

fn required_string(params: &Map<String, Value>, key: &str, max_length: usize) -> Result<String> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{key} must be a string"))?;
    if value.len() > max_length {
        bail!("{key} is too long");
    }
    Ok(value.to_string())
}

fn required_index(params: &Map<String, Value>, key: &str) -> Result<usize> {
    required_u64(params, key)?
        .try_into()
        .map_err(|_| anyhow!("{key} is out of range"))
}

fn required_u64(params: &Map<String, Value>, key: &str) -> Result<u64> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("{key} must be an unsigned integer"))
}

fn required_bool_value(value: &Value, key: &str) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| anyhow!("{key} must be a boolean"))
}

fn search_snapshot(
    search: &PaginatedSearch,
    session: &str,
    query: &str,
    page: usize,
    generation: u64,
) -> Value {
    let start = page.saturating_sub(1).saturating_mul(search.page_size);
    let end = start
        .saturating_add(search.page_size)
        .min(search.results.len());
    let results = search.results[start.min(end)..end]
        .iter()
        .map(track_value)
        .collect::<Vec<_>>();
    let has_more = end < search.results.len() || !search.exhausted;
    json!({"session": session, "query": query, "page": page, "generation": generation, "results": results, "has_more": has_more})
}

fn track_value(track: &SearchResult) -> Value {
    json!({
        "id": track.id,
        "title": track.title,
        "duration": track.duration,
        "channel": track.channel,
        "views": track.views,
        "published": track.published,
    })
}

fn read_frame(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut prefix = [0_u8; 4];
    let mut offset = 0;
    while offset < prefix.len() {
        match reader.read(&mut prefix[offset..])? {
            0 if offset == 0 => return Ok(None),
            0 => bail!("stdin closed in a frame header"),
            count => offset += count,
        }
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_SIZE {
        bail!("request frame size is invalid");
    }
    let mut frame = vec![0_u8; length];
    reader
        .read_exact(&mut frame)
        .context("stdin closed in a request frame")?;
    Ok(Some(frame))
}

fn parse_request(frame: &[u8]) -> std::result::Result<Request, (Option<u64>, String)> {
    let value: Value = serde_json::from_slice(frame)
        .map_err(|_| (None, "request is not valid JSON".to_string()))?;
    let id = value.get("id").and_then(Value::as_u64);
    let Some(id) = id.filter(|id| *id > 0) else {
        return Err((None, "id must be a positive unsigned integer".to_string()));
    };
    if value.get("version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err((Some(id), "unsupported protocol version".to_string()));
    }
    let Some(command) = value.get("command").and_then(Value::as_str) else {
        return Err((Some(id), "command must be a string".to_string()));
    };
    let Some(params) = value.get("params").and_then(Value::as_object) else {
        return Err((Some(id), "params must be an object".to_string()));
    };
    Ok(Request {
        id,
        command: command.to_owned(),
        params: params.clone(),
    })
}

fn reserve_request(requests: &Arc<Mutex<HashSet<u64>>>, id: u64) -> bool {
    requests
        .lock()
        .map(|mut requests| requests.insert(id))
        .unwrap_or(false)
}

fn finish_request(
    output: &Arc<Mutex<BufWriter<io::Stdout>>>,
    requests: &Arc<Mutex<HashSet<u64>>>,
    id: u64,
    result: Result<Value>,
) -> Result<()> {
    let response = match result {
        Ok(result) => json!({"version": PROTOCOL_VERSION, "id": id, "ok": true, "result": result}),
        Err(error) => error_response(Some(id), public_error(&error)),
    };
    let write_result = write_frame(output, &response);
    if let Ok(mut requests) = requests.lock() {
        requests.remove(&id);
    }
    write_result
}

fn error_response(id: Option<u64>, error: impl AsRef<str>) -> Value {
    let mut response = json!({"version": PROTOCOL_VERSION, "ok": false, "error": error.as_ref()});
    if let Some(id) = id {
        response["id"] = json!(id);
    }
    response
}

fn public_error(error: &anyhow::Error) -> String {
    let text = error.to_string();
    let normalized = text.to_ascii_lowercase();
    if normalized.contains("cancelled") {
        "request cancelled".to_string()
    } else if normalized.contains("media preparation is busy") {
        "a video is already loading; wait for it to finish".to_string()
    } else if normalized.contains("search") || normalized.contains("yt-dlp search") {
        "cannot search right now".to_string()
    } else if normalized.contains("yt-dlp")
        || normalized.contains("ffmpeg")
        || normalized.contains("ffprobe")
        || normalized.contains("media")
    {
        "cannot prepare this video".to_string()
    } else {
        text
    }
}

fn write_event(output: &Arc<Mutex<BufWriter<io::Stdout>>>, event: Value) -> Result<()> {
    write_frame(output, &event)
}

fn write_frame(output: &Arc<Mutex<BufWriter<io::Stdout>>>, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value).context("cannot encode protocol frame")?;
    if bytes.len() > MAX_FRAME_SIZE {
        bail!("response frame exceeds the protocol limit");
    }
    let length = u32::try_from(bytes.len()).context("protocol response is too large")?;
    let mut output = output
        .lock()
        .map_err(|_| anyhow!("protocol output is unavailable"))?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

fn reserve_media_slot(slots: &Arc<Mutex<usize>>) -> bool {
    let Ok(mut active) = slots.lock() else {
        return false;
    };
    if *active >= MEDIA_JOB_LIMIT {
        return false;
    }
    *active += 1;
    true
}

fn release_media_slot(slots: &Arc<Mutex<usize>>) {
    if let Ok(mut active) = slots.lock() {
        *active = active.saturating_sub(1);
    }
}

fn ensure_known<T>(value: Option<T>, message: &'static str) -> Result<T> {
    value.ok_or_else(|| anyhow!(message))
}

struct Request {
    id: u64,
    command: String,
    params: Map<String, Value>,
}

struct SearchJob {
    session: String,
    query: String,
    page: usize,
    request_id: u64,
    generation: u64,
    sort_by_date: bool,
    cancelled: Arc<AtomicBool>,
    search: Arc<Mutex<PaginatedSearch>>,
}

struct BackendState {
    queue: Queue,
    current: Option<SearchResult>,
    playback_id: u64,
    owner: Option<String>,
    config: Config,
    revision: u64,
    // Least recently requested session first.
    searches: VecDeque<SearchCacheEntry>,
}

struct SearchCacheEntry {
    session: String,
    // Only an outstanding job keeps this token alive.
    cancelled: Weak<AtomicBool>,
    query: String,
    page_size: usize,
    filter_shorts: bool,
    sort_by_date: bool,
    search: Arc<Mutex<PaginatedSearch>>,
}

impl BackendState {
    fn new(config: Config) -> Self {
        Self {
            queue: Queue::new(),
            current: None,
            playback_id: 0,
            owner: None,
            config,
            revision: 0,
            searches: VecDeque::new(),
        }
    }

    fn snapshot(&self) -> Value {
        let queue = self.queue.iter().map(track_value).collect::<Vec<_>>();
        json!({
            "queue": queue,
            "current": self.current.as_ref().map(track_value),
            "playback_id": self.playback_id,
            "owner": self.owner,
            "revision": self.revision,
            "settings": {
                "include_shorts": self.config.include_shorts,
                "sort_by_date": self.config.sort_by_date,
                "results_per_page": self.config.results_per_page,
                "auto_play_queue": self.config.auto_play_queue,
            },
        })
    }

    fn start_playback(&mut self, track: SearchResult, owner: String) {
        self.playback_id = self.playback_id.wrapping_add(1);
        self.current = Some(track);
        self.owner = Some(owner);
    }

    fn advance(&mut self, owner: String) {
        self.current = self.queue.pop_front();
        self.playback_id = self.playback_id.wrapping_add(1);
        self.owner = self.current.as_ref().map(|_| owner);
    }
}

enum ProcessEnd {
    Success,
    Failed(i32),
    Cancelled,
    TimedOut,
}

fn ensure_active(cancelled: &AtomicBool, shutting_down: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::SeqCst) || shutting_down.load(Ordering::SeqCst) {
        bail!("media preparation cancelled");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BackendState, CachedMedia, MAX_FRAME_SIZE, MAX_SEARCH_SESSIONS, MediaCache, cached_media,
        cancel_search, output_is_truncated, parse_request, parse_track, prepare_or_reuse_media,
        prepare_search, prune_media_cache, read_frame, search_snapshot, validate_video_id,
    };
    use crate::config::Config;
    use crate::search::{PaginatedSearch, SearchResult};
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn frame_reader_accepts_valid_json_payload_and_rejects_oversize() {
        let payload = br#"{"version":1,"id":1,"command":"ping","params":{}}"#;
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(payload);
        assert_eq!(
            read_frame(&mut Cursor::new(framed)).unwrap(),
            Some(payload.to_vec())
        );

        let oversized = ((MAX_FRAME_SIZE + 1) as u32).to_be_bytes();
        assert!(read_frame(&mut Cursor::new(oversized)).is_err());
    }

    #[test]
    fn request_parser_requires_positive_numeric_ids_and_object_params() {
        let valid = parse_request(br#"{"version":1,"id":1,"command":"ping","params":{}}"#).unwrap();
        assert_eq!(valid.id, 1);
        assert!(parse_request(br#"{"version":1,"id":"1","command":"ping","params":{}}"#).is_err());
        assert!(parse_request(br#"{"version":1,"id":0,"command":"ping","params":{}}"#).is_err());
        assert!(parse_request(br#"{"version":1,"id":1,"command":"ping","params":[]}"#).is_err());
    }

    #[test]
    fn client_tracks_require_all_string_fields_and_valid_video_ids() {
        let valid = json!({
            "id": "abcdefghijk", "title": "title", "duration": "1:00",
            "channel": "channel", "views": "5", "published": "today"
        });
        assert!(parse_track(Some(&valid)).is_ok());
        assert!(
            parse_track(Some(
                &json!({"id": "https://youtube.com/watch?v=abcdefghijk"})
            ))
            .is_err()
        );
        assert!(validate_video_id("abcdefghij!").is_err());
    }

    #[test]
    fn queue_snapshots_preserve_current_and_ignore_stale_playback_events() {
        let current = SearchResult {
            id: "abcdefghijk".to_string(),
            title: "current".to_string(),
            duration: "1:00".to_string(),
            channel: "channel".to_string(),
            views: "5".to_string(),
            published: "today".to_string(),
        };
        let mut state = BackendState::new(Config::default());
        state.start_playback(current.clone(), "tab-a".to_string());
        state.queue.push_back(current);
        let stale = json!({"owner":"tab-a","playback_id":0});
        let stale_params = stale.as_object().unwrap();
        let state_snapshot = state.snapshot();
        assert_eq!(state_snapshot["queue"].as_array().unwrap().len(), 1);
        assert_eq!(state_snapshot["current"]["id"], "abcdefghijk");
        assert_eq!(stale_params["playback_id"], 0);
        assert_eq!(state.playback_id, 1);
    }

    #[test]
    fn search_cache_evicts_oldest_idle_session_and_preserves_active_pagination() {
        let mut state = BackendState::new(Config::default());
        let params = |session: usize, page: usize| json!({"session": session.to_string(), "query": "query", "page": page});
        let first = params(0, 1);
        let active = prepare_search(first.as_object().unwrap(), 1, &mut state).unwrap();
        for session in 1..=MAX_SEARCH_SESSIONS {
            let request = params(session, 1);
            prepare_search(request.as_object().unwrap(), 2, &mut state).unwrap();
        }

        assert_eq!(state.searches.len(), MAX_SEARCH_SESSIONS);
        assert!(!state.searches.iter().any(|entry| entry.session == "1"));
        assert!(!active.cancelled.load(Ordering::SeqCst));
        let next = params(0, 2);
        let next = prepare_search(next.as_object().unwrap(), 3, &mut state).unwrap();
        assert!(Arc::ptr_eq(&active.search, &next.search));
        assert!(active.cancelled.load(Ordering::SeqCst));
        assert_eq!(
            cancel_search(first.as_object().unwrap(), &state).unwrap()["cancelled"],
            true
        );
        assert!(next.cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn search_snapshot_uses_one_based_pages_and_string_result_fields() {
        let mut search = PaginatedSearch::new("query", 1, false);
        search.exhausted = true;
        search.results.push(SearchResult {
            id: "abcdefghijk".to_string(),
            title: "title".to_string(),
            duration: "1:00".to_string(),
            channel: "channel".to_string(),
            views: "5".to_string(),
            published: "today".to_string(),
        });
        let snapshot = search_snapshot(&search, "tab-a", "query", 1, 0);
        assert_eq!(snapshot["page"], 1);
        assert_eq!(snapshot["results"][0]["published"], "today");
        assert_eq!(snapshot["has_more"], false);
    }

    #[test]
    fn media_cache_pruning_keeps_the_new_file_and_enforces_entry_limit() {
        let cache = tempfile::tempdir().unwrap();
        let mut current = None;
        for index in 0..13 {
            let path = cache.path().join(format!("m{index:016x}.mp4"));
            fs::write(&path, b"mp4").unwrap();
            current = Some(path);
        }
        let current = current.unwrap();
        prune_media_cache(cache.path(), &current).unwrap();
        let files = fs::read_dir(cache.path()).unwrap().count();
        assert_eq!(files, 12);
        assert!(current.exists());
    }

    #[test]
    fn media_duration_guard_allows_small_mux_differences_and_rejects_truncation() {
        assert!(!output_is_truncated(10.0, 9.5));
        assert!(output_is_truncated(120.0, 90.0));
    }

    #[test]
    fn post_reservation_cache_hit_reuses_media_without_starting_a_job() {
        let cache_dir = tempfile::tempdir().unwrap();
        let video_id = "abcdefghijk";
        let media_id = "m0000000000000001";
        let path = cache_dir.path().join(format!("{media_id}.mp4"));
        fs::write(&path, b"prepared mp4").unwrap();
        let cache = Arc::new(Mutex::new(MediaCache::default()));
        cache.lock().unwrap().by_video_id.insert(
            video_id.to_string(),
            CachedMedia {
                media_id: media_id.to_string(),
                path: path.clone(),
            },
        );
        let counter = AtomicU64::new(0);
        let cancelled = AtomicBool::new(false);
        let shutting_down = AtomicBool::new(false);

        let result = prepare_or_reuse_media(
            video_id,
            cache_dir.path(),
            &cache,
            &counter,
            &cancelled,
            &shutting_down,
        )
        .unwrap();

        assert_eq!(result["media_id"], media_id);
        assert_eq!(
            result["path"],
            path.canonicalize().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(fs::read_dir(cache_dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn cached_media_discards_missing_and_symlink_entries() {
        let cache_dir = tempfile::tempdir().unwrap();
        let cache_root = cache_dir.path().canonicalize().unwrap();
        let cache = Arc::new(Mutex::new(MediaCache::default()));
        let missing_video_id = "abcdefghijk";
        let missing = CachedMedia {
            media_id: "m0000000000000001".to_string(),
            path: cache_dir.path().join("m0000000000000001.mp4"),
        };
        cache
            .lock()
            .unwrap()
            .by_video_id
            .insert(missing_video_id.to_string(), missing);

        assert!(
            cached_media(missing_video_id, &cache_root, &cache)
                .unwrap()
                .is_none()
        );
        assert!(
            !cache
                .lock()
                .unwrap()
                .by_video_id
                .contains_key(missing_video_id)
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let symlink_video_id = "lmnopqrstuv";
            let target = cache_dir.path().join("target.mp4");
            fs::write(&target, b"prepared mp4").unwrap();
            let link = cache_dir.path().join("m0000000000000002.mp4");
            symlink(target, &link).unwrap();
            cache.lock().unwrap().by_video_id.insert(
                symlink_video_id.to_string(),
                CachedMedia {
                    media_id: "m0000000000000002".to_string(),
                    path: link,
                },
            );

            assert!(
                cached_media(symlink_video_id, &cache_root, &cache)
                    .unwrap()
                    .is_none()
            );
            assert!(
                !cache
                    .lock()
                    .unwrap()
                    .by_video_id
                    .contains_key(symlink_video_id)
            );
        }
    }
}
