# Local web interface

## Scope

`youtui --web` starts Phoenix on loopback and opens the browser.
The browser plays video through a native video element.
Rust supplies search, queue state, shared settings, and media preparation.
The existing terminal interface remains available with `youtui`.

The first version serves completed MP4 files with audio and video.
The page shows preparation progress before playback starts.
Adaptive streaming and saved playlists are later extensions.

## Ownership

- The root agent owns `src/main.rs`, `src/web_launcher.rs`, repository documentation, CI, and integration review.
- The Rust worker owns `src/web_backend/` and backend tests.
- The Phoenix worker owns the scaffold, bridge, controllers, authentication, and server tests.
- The Rust worker owns the interface after the backend compiles.

Workers use GPT Luna. The root agent reviews their changes and resolves integration errors.
Workers use Serena for symbol discovery and `apply_patch` for file edits.
Shell commands start with `rtk`. File discovery uses `fd`.

## Process model

The public Rust launcher starts a Phoenix release or the development project.
Phoenix starts the same Rust executable with `--backend-stdio` through an Elixir Port.
The worker exits when stdin closes and cancels its subprocesses.
Protocol output uses stdout. Diagnostic output uses stderr.
The launcher waits for `/health` before it opens `/start?token=...`.
Ctrl+C stops the launcher, Phoenix, the worker, and media subprocesses.

Launcher environment:

- `YOUTUI_BIN`: absolute path to the Rust executable.
- `YOUTUI_PORT`: requested HTTP port, default `4000`.
- `YOUTUI_WEB_TOKEN`: random bootstrap token for the browser session.
- `YOUTUI_WEB_RELEASE`: optional path to a Phoenix release directory.
- `YOUTUI_WEB_DIR`: optional path to the Phoenix source project.
- `YOUTUI_CONFIG_PATH`: optional settings path for isolated tests.

Phoenix reads runtime values and binds to `127.0.0.1`.
`/start` exchanges the bootstrap token for a signed session and redirects to `/`.
The UI and media routes require that session.
The health route returns readiness without user data.
Phoenix retains origin and CSRF checks.

## Rust protocol version 1

Each JSON frame has a four-byte unsigned big-endian length prefix.
The maximum frame size is 1 MiB.
An Elixir Port uses `:binary`, `{:packet, 4}`, and `:exit_status`.

Request:

```json
{"version":1,"id":1,"command":"state","params":{}}
```

Success:

```json
{"version":1,"id":1,"ok":true,"result":{}}
```

Error:

```json
{"version":1,"id":1,"ok":false,"error":"cannot prepare this video"}
```

Event:

```json
{"version":1,"event":"state_changed","result":{}}
```

The worker emits `{"version":1,"event":"ready","cache_dir":"/absolute/cache/path"}` after initialization.
Phoenix broadcasts `{:backend_event, event_map}` to bridge subscribers.
The public bridge API is `Youtui.Backend.request(command, params \\ %{}, timeout \\ 30_000)`.
It returns `{:ok, result_map}` or `{:error, message}`.
`Youtui.Backend.subscribe()` subscribes the current process.
The GenServer name is `Youtui.Backend`.

Search result fields are `id`, `title`, `duration`, `channel`, `views`, and `published`.
All fields are strings. `id` is a validated YouTube video ID.

Snapshot fields:

```json
{
  "revision": 0,
  "queue": [],
  "current": null,
  "playback_id": 0,
  "owner": null,
  "settings": {
    "include_shorts": false,
    "sort_by_date": false,
    "results_per_page": 20,
    "auto_play_queue": true
  }
}
```

Commands:

| Command | Parameters | Result |
| --- | --- | --- |
| `ping` | none | `{"version":1}` |
| `state` | none | snapshot |
| `search` | `session`, `query`, `page` (one-based), optional `generation` | search snapshot |
| `cancel_search` | `session` | `{"cancelled":true}` |
| `queue_add` | `track` (search result) | snapshot |
| `queue_remove` | `index` (zero-based), optional `expected_revision` | snapshot |
| `queue_clear` | none | snapshot |
| `queue_play` | `index`, `owner`, optional `expected_revision` | snapshot |
| `play` | `track`, `owner` | snapshot |
| `skip` | `owner`, `playback_id` | snapshot |
| `playback_ended` | `owner`, `playback_id` | snapshot |
| `settings_update` | supported settings fields | snapshot |
| `prepare_media` | `video_id` | `media_id`, absolute `path`, `mime` |
| `shutdown` | none | `{"shutdown":true}` |

Search snapshots contain `session`, `query`, `page`, `generation`, `results`, and `has_more`.
The worker emits `search_progress` events with `request_id` and the partial search snapshot in `result`.
Searches cancel independently by session. New searches replace old searches for that session.
The backend reuses pagination state for later pages of the same query.
The worker keeps command input responsive during search and media preparation.

Queue changes and settings changes emit `state_changed` events with the full snapshot.
`queue_add` appends an upcoming track without automatic playback.
`queue_clear` removes upcoming tracks and preserves the current track.
`play` starts a new occurrence and increments `playback_id`.
`queue_play` removes the selected upcoming track and makes it current.
`skip` advances regardless of the automatic playback setting.
`playback_ended` advances only when automatic playback is enabled.
Both commands ignore events with an old owner or playback ID.
An ended track clears the current track when automatic playback is disabled.
Duplicate video IDs remain valid queue entries.
The web queue holds up to 100 upcoming tracks. Each track's encoded metadata is limited to 8 KiB.
Snapshot revisions increase on every state change.
The interface rejects older snapshot revisions.
Queue selection and removal use `expected_revision` to reject changes from another tab.
The worker checks this value before it changes the queue.
Queue buttons carry the revision from their browser render, not a later server snapshot.
The skip button carries the displayed playback ID for the same reason.

## Media and browser contract

Rust uses yt-dlp and ffmpeg to prepare compatible MP4 files in a managed temporary directory.
The format selects H.264 video and AAC audio, with a source height limit of 720p.
Both the format and media output require validation.
The worker constructs YouTube URLs from validated IDs.
The worker rejects arbitrary source URLs and filesystem paths from client commands.
Preparation has a timeout and cancellation on shutdown.
The worker bounds its media cache and concurrent preparation jobs.

Phoenix registers completed media through `Youtui.Media.register(result_map)`.
This function returns `{:ok, "/media/:media_id"}` or `{:error, message}`.
The URL requires a browser session.
The controller supports GET, HEAD, byte ranges, and `416` for invalid ranges.
The controller serves only registered completed files under the worker cache directory.
It never accepts a filesystem path or an upstream URL from the browser.

The main LiveView module is `YoutuiWeb.PlayerLive` at `/`.
The interface uses backend snapshots and retains browser ownership of the video DOM.
The player hook is `YoutuiPlayer` in `web/assets/js/player.js`.
The Phoenix worker imports the hook into `web/assets/js/app.js`.
The Phoenix worker imports `youtui.css` into `web/assets/css/app.css`.
The interface worker exports the hook as the default JavaScript export.

The hook receives `load_video` with `url` and `playback_id`.
It receives `stop_video` to clear the source.
It sends `video_ended` and `video_error` with the playback ID.
The hook catches autoplay errors and shows a manual play control.
The video element remains mounted during search, queue updates, and settings changes.

## Acceptance

- `youtui --web --no-open` starts without a TTY or mpv.
- The browser can search, cancel, paginate, play, and manage its queue.
- An actual MP4 plays with audio and supports seeking.
- Stale playback events cannot advance the current queue.
- Closing worker stdin cancels active child processes and exits.
- Unauthenticated media requests fail and invalid byte ranges return `416`.
- Existing Rust tests pass. New protocol and Phoenix tests pass.
- The browser UI is usable at desktop and narrow viewport widths.
- A production Phoenix release boots through the Rust launcher.

## Verification completed

- Rust: 282 tests passed. One existing benchmark test is ignored.
- Phoenix: 30 tests passed. Formatting and strict compilation passed.
- Strict Clippy and the Rust release build passed.
- Source startup and the Phoenix production release both passed launcher checks.
- The production release started without Mix or Elixir on `PATH`.
- A public YouTube video passed real media preparation and codec validation.
- Browser checks used local media fixtures with audio and video.
- Playback, seeking, queue advancement, pagination, and tab ownership passed browser checks.
- Search and queue updates preserved the video element.
- A 390-pixel viewport had no horizontal overflow.
- Worker restart cleared playback and invalidated old media URLs.
- Ctrl+C stopped the test servers and workers.
