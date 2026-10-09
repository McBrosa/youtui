defmodule YoutuiWeb.PlayerLiveTest do
  use YoutuiWeb.ConnCase, async: false

  import Phoenix.LiveViewTest

  test "renders the search, local player, queue, and settings workspace", %{conn: conn} do
    conn = init_test_session(conn, %{youtui_session: "liveview-test-session"})
    {:ok, view, _html} = live(conn, "/")

    assert has_element?(view, "#workspace-title.sr-only", "youtui")
    refute has_element?(view, ".yt-heading-row")
    refute has_element?(view, ".yt-heading-copy")
    assert has_element?(view, "#search-form")
    assert has_element?(view, ".yt-search-meta .yt-key-hint", "Enter")
    assert has_element?(view, "#search-query")
    assert has_element?(view, "#player-surface[phx-hook='YoutuiPlayer']")
    assert has_element?(view, "#player-video[controls]")
    assert has_element?(view, "#queue-empty")
    assert has_element?(view, "#settings-toggle")
    assert has_element?(view, ".yt-search-button[aria-label='Search YouTube videos']")
    assert has_element?(view, "#include-shorts-toggle[role='switch'][aria-checked='false']")
    assert has_element?(view, "#sort-by-newest-toggle[role='switch'][aria-checked='false']")
    assert has_element?(view, "#autoplay-queue-toggle[role='switch'][aria-checked='true']")
    assert has_element?(view, ".yt-local-label", "Local session")

    named = track("named-actions", "Quiet song")
    send(view.pid, {:search_result, 0, {:ok, %{"results" => [named]}}})
    _ = :sys.get_state(view.pid)

    assert has_element?(
             view,
             "#result-named-actions .yt-play-button[aria-label='Play Quiet song']"
           )

    assert has_element?(
             view,
             "#result-named-actions .yt-add-button[aria-label='Add Quiet song to queue']"
           )
  end

  test "theater mode toggles without changing player, results, queue, or session state", %{
    conn: conn
  } do
    view = live_player(conn, "liveview-theater-mode")
    current = track("remote-current", "Remote current")
    queued = track("queued-theater", "Queued video")
    result = track("result-theater", "Search result")

    state = snapshot(4, current, "another-session") |> Map.put("queue", [queued])
    send(view.pid, {:initial_state, 0, {:ok, state}})
    send(view.pid, {:search_result, 0, {:ok, %{"results" => [result]}}})
    _ = :sys.get_state(view.pid)

    refute has_element?(view, ".yt-workspace--theater")
    assert has_element?(view, "#theater-toggle[aria-pressed='false']")
    assert has_element?(view, "#player-card > #player-surface[phx-hook='YoutuiPlayer']")
    assert has_element?(view, "#player-card #player-video[controls]")
    assert has_element?(view, "#result-result-theater")
    assert has_element?(view, "#queue-0")
    assert has_element?(view, "#play-current-here")

    before = :sys.get_state(view.pid).socket.assigns

    view |> element("#theater-toggle") |> render_click()

    assert has_element?(view, ".yt-workspace.yt-workspace--theater")
    assert has_element?(view, "#theater-toggle[aria-pressed='true']")
    assert has_element?(view, "#player-card > #player-surface[phx-hook='YoutuiPlayer']")
    assert has_element?(view, "#player-card #player-video[controls]")
    assert has_element?(view, "#result-result-theater")
    assert has_element?(view, "#queue-0")
    assert has_element?(view, "#play-current-here")

    view |> element("#theater-toggle") |> render_click()

    assert has_element?(view, "#theater-toggle[aria-pressed='false']")
    refute has_element?(view, ".yt-workspace--theater")
    after_toggle = :sys.get_state(view.pid).socket.assigns

    assert after_toggle.owner == before.owner
    assert after_toggle.result_tracks == before.result_tracks
    assert after_toggle.queue_items == before.queue_items
    assert after_toggle.current == before.current
    assert after_toggle.playback_id == before.playback_id
    assert after_toggle.playback_owner == before.playback_owner
    assert after_toggle.snapshot == before.snapshot
    assert is_nil(after_toggle.preparing_video_id)
    assert is_nil(after_toggle.preparing_track)
  end

  test "theater mode preserves the active preparation guard and survives snapshots", %{conn: conn} do
    view = live_player(conn, "liveview-theater-preparing")
    selected = track("selected-theater", "Selected video")
    ignored = track("ignored-theater", "Ignored video")

    send(view.pid, {:initial_state, 0, {:ok, snapshot(0, nil, nil)}})
    send(view.pid, {:search_result, 0, {:ok, %{"results" => [selected, ignored]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "play_result", %{"track" => Jason.encode!(selected)})
      assert await_backend_prepare_request_count(1) == 1

      preparing = :sys.get_state(view.pid).socket.assigns
      assert preparing.preparing_video_id == selected["id"]
      generation = preparing.play_generation

      view |> element("#theater-toggle") |> render_click()
      assert has_element?(view, ".yt-workspace--theater")
      assert has_element?(view, "#media-preparing", "Selected video")
      assert has_element?(view, "#player-surface[phx-hook='YoutuiPlayer']")

      render_click(view, "play_result", %{"track" => Jason.encode!(ignored)})
      assert :sys.get_state(view.pid).socket.assigns.play_generation == generation
      assert backend_prepare_request_count() == 1

      refreshed = snapshot(1, nil, nil) |> Map.put("queue", [track("snapshot-queue")])
      send(view.pid, {:backend_event, %{"event" => "state_changed", "result" => refreshed}})
      _ = :sys.get_state(view.pid)

      assert has_element?(view, ".yt-workspace--theater")
      assert has_element?(view, "#theater-toggle[aria-pressed='true']")
      assert has_element?(view, "#media-preparing", "Selected video")
      assert has_element?(view, "#queue-0")

      after_snapshot = :sys.get_state(view.pid).socket.assigns
      assert after_snapshot.preparing_video_id == selected["id"]
      assert after_snapshot.preparing_track == selected
      assert after_snapshot.play_generation == generation
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "setting switches send string values for both states", %{conn: conn} do
    view = live_player(conn, "liveview-switch-values")
    :ok = :sys.suspend(Youtui.Backend)
    initial_request_count = length(backend_setting_requests())

    try do
      switches = [
        {"include-shorts-toggle", "include_shorts", false},
        {"sort-by-newest-toggle", "sort_by_date", false},
        {"autoplay-queue-toggle", "auto_play_queue", true}
      ]

      Enum.with_index(switches, 1)
      |> Enum.each(fn {{id, field, initial_value}, revision} ->
        first_value = not initial_value
        first_value_text = to_string(first_value)
        second_value_text = to_string(initial_value)

        assert has_element?(view, "##{id}[phx-value-enabled='#{first_value_text}']")
        view |> element("##{id}") |> render_click(%{"value" => ""})

        assert await_backend_setting_request_count(initial_request_count + revision * 2 - 1) ==
                 initial_request_count + revision * 2 - 1

        assert Enum.at(backend_setting_requests(), initial_request_count + revision * 2 - 2) ==
                 %{field => first_value}

        latest = snapshot(revision, nil, nil)
        settings = Map.put(latest["settings"], field, first_value)

        send(
          view.pid,
          {:backend_result, 0, :settings_result, {:ok, Map.put(latest, "settings", settings)}}
        )

        _ = :sys.get_state(view.pid)

        assert has_element?(view, "##{id}[role='switch'][aria-checked='#{first_value_text}']")
        assert has_element?(view, "##{id}[phx-value-enabled='#{second_value_text}']")
        view |> element("##{id}") |> render_click(%{"value" => ""})

        assert await_backend_setting_request_count(initial_request_count + revision * 2) ==
                 initial_request_count + revision * 2

        assert Enum.at(backend_setting_requests(), initial_request_count + revision * 2 - 1) ==
                 %{field => initial_value}

        latest = snapshot(revision + 1, nil, nil)
        settings = Map.put(latest["settings"], field, initial_value)

        send(
          view.pid,
          {:backend_result, 0, :settings_result, {:ok, Map.put(latest, "settings", settings)}}
        )

        _ = :sys.get_state(view.pid)
      end)
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "preparation keeps its selected title visible during another search", %{conn: conn} do
    view = live_player(conn, "liveview-preparing-title")
    selected = track("selected-video", "Selected title")

    send(view.pid, {:search_result, 0, {:ok, %{"results" => [selected]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "play_result", %{"track" => Jason.encode!(selected)})
      assert await_backend_prepare_request_count(1) == 1

      view
      |> element("#search-form")
      |> render_submit(%{"search" => %{"query" => "another search"}})

      assert has_element?(view, "#media-preparing", "Selected title")
      assert has_element?(view, "#media-preparing", "Other videos wait until this finishes.")
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "retry locks the same failed video and ignores another retry click", %{conn: conn} do
    view = live_player(conn, "liveview-retry-lock")
    failed = track("retry-video", "Retry title")

    send(view.pid, {:search_result, 0, {:ok, %{"results" => [failed]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "play_result", %{"track" => Jason.encode!(failed)})
      assert await_backend_prepare_request_count(1) == 1
      send(view.pid, {:play_prepared, 0, 1, failed, {:error, "decoder unavailable"}})
      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#player-error.yt-error")
      assert has_element?(view, ".yt-error-title", "Video preparation failed")
      assert has_element?(view, ".yt-error-message", "decoder unavailable")
      assert has_element?(view, "#retry-video.yt-retry-button")

      render_click(view, "retry_preparation", %{
        "video_id" => failed["id"],
        "failure_generation" => "1"
      })

      state = :sys.get_state(view.pid).socket.assigns
      assert state.preparing_video_id == failed["id"]
      assert state.preparing_track == failed
      assert state.pending_play.kind == :track
      generation = state.play_generation
      assert has_element?(view, "#media-preparing", "Retry title")

      render_click(view, "retry_preparation", %{
        "video_id" => failed["id"],
        "failure_generation" => "1"
      })

      assert :sys.get_state(view.pid).socket.assigns.play_generation == generation
      assert backend_prepare_request_count() == 1
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "a queue retry retains its queue index and revision", %{conn: conn} do
    view = live_player(conn, "liveview-queue-retry")
    queued = track("queued-retry", "Queued retry")
    snapshot = snapshot(3, nil, nil) |> Map.put("queue", [queued])

    send(view.pid, {:initial_state, 0, {:ok, snapshot}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "queue_play", %{"index" => "0", "revision" => "3"})
      assert await_backend_prepare_request_count(1) == 1
      send(view.pid, {:queue_play_prepared, 0, 1, 0, 3, queued, {:error, "media unavailable"}})
      _ = :sys.get_state(view.pid)

      failure = :sys.get_state(view.pid).socket.assigns.failed_preparation
      assert failure.kind == :queue
      assert failure.index == 0
      assert failure.expected_revision == 3
      assert failure.track == queued

      render_click(view, "retry_preparation", %{
        "video_id" => queued["id"],
        "failure_generation" => "1"
      })

      pending = :sys.get_state(view.pid).socket.assigns.pending_play
      assert pending.kind == :queue
      assert pending.index == 0
      assert pending.expected_revision == 3
      assert pending.track == queued
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "a newer playback owner clears the failed preparation retry", %{conn: conn} do
    view = live_player(conn, "liveview-stale-retry-owner")
    failed = track("old-retry", "Old retry")

    send(view.pid, {:initial_state, 0, {:ok, snapshot(1, nil, nil)}})
    send(view.pid, {:search_result, 0, {:ok, %{"results" => [failed]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "play_result", %{"track" => Jason.encode!(failed)})
      assert await_backend_prepare_request_count(1) == 1
      send(view.pid, {:play_prepared, 0, 1, failed, {:error, "media unavailable"}})
      _ = :sys.get_state(view.pid)
      assert has_element?(view, "#retry-video")

      newer = snapshot(2, track("remote-current"), "another-session")
      send(view.pid, {:backend_event, %{"event" => "state_changed", "result" => newer}})
      _ = :sys.get_state(view.pid)

      refute has_element?(view, "#retry-video")
      refute has_element?(view, "#media-preparing")
      assert is_nil(:sys.get_state(view.pid).socket.assigns.failed_preparation)

      render_click(view, "retry_preparation", %{
        "video_id" => failed["id"],
        "failure_generation" => "1"
      })

      assert :sys.get_state(view.pid).socket.assigns.play_generation == 1
      assert backend_prepare_request_count() == 1
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "a worker restart clears the failed preparation retry", %{conn: conn} do
    view = live_player(conn, "liveview-restart-retry")
    failed = track("restart-retry", "Restart retry")

    :ok = :sys.suspend(Youtui.Backend)

    try do
      send(view.pid, {:search_result, 0, {:ok, %{"results" => [failed]}}})
      _ = :sys.get_state(view.pid)
      render_click(view, "play_result", %{"track" => Jason.encode!(failed)})
      assert await_backend_prepare_request_count(1) == 1
      send(view.pid, {:play_prepared, 0, 1, failed, {:error, "media unavailable"}})
      _ = :sys.get_state(view.pid)
      assert has_element?(view, "#retry-video")

      send(view.pid, {:backend_event, %{"event" => "disconnected"}})
      _ = :sys.get_state(view.pid)

      refute has_element?(view, "#retry-video")
      assert is_nil(:sys.get_state(view.pid).socket.assigns.failed_preparation)
      assert :sys.get_state(view.pid).socket.assigns.backend_epoch == 1
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "current playback preparation offers a guarded retry without auto-retrying snapshots", %{
    conn: conn
  } do
    view = live_player(conn, "liveview-current-retry")
    owner = :sys.get_state(view.pid).socket.assigns.owner
    current = track("current-retry", "Current retry")
    state = snapshot(7, current, owner)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      send(view.pid, {:initial_state, 0, {:ok, state}})
      _ = :sys.get_state(view.pid)

      send(
        view.pid,
        {:prepare_current_result, 0, 7, {:error, {:media_preparation, "media unavailable"}}}
      )

      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#retry-video")
      assert :sys.get_state(view.pid).socket.assigns.failed_preparation.kind == :current

      newer_revision = Map.put(state, "revision", 8)
      send(view.pid, {:backend_event, %{"event" => "state_changed", "result" => newer_revision}})
      _ = :sys.get_state(view.pid)

      assert is_nil(:sys.get_state(view.pid).socket.assigns.preparing_current)
      assert has_element?(view, "#retry-video")

      failure = :sys.get_state(view.pid).socket.assigns.failed_preparation

      render_click(view, "retry_preparation", %{
        "video_id" => current["id"],
        "failure_generation" => "0"
      })

      pending = :sys.get_state(view.pid).socket.assigns.pending_play
      assert pending.kind == :current
      assert pending.track == current
      assert :sys.get_state(view.pid).socket.assigns.preparing_video_id == current["id"]

      retry_generation = :sys.get_state(view.pid).socket.assigns.play_generation

      completion =
        {:retry_prepared, 0, retry_generation, failure, newer_revision, {:ok, "/media/current"}}

      send(view.pid, completion)
      _ = :sys.get_state(view.pid)

      assert :sys.get_state(view.pid).socket.assigns.loaded_playback_id == 7
      assert is_nil(:sys.get_state(view.pid).socket.assigns.preparing_video_id)
      assert is_nil(:sys.get_state(view.pid).socket.assigns.failed_preparation)

      send(view.pid, completion)
      _ = :sys.get_state(view.pid)
      assert :sys.get_state(view.pid).socket.assigns.loaded_playback_id == 7
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "media registration errors do not offer Retry", %{conn: conn} do
    view = live_player(conn, "liveview-registration-error")
    failed = track("registration-error", "Registration error")

    send(view.pid, {:search_result, 0, {:ok, %{"results" => [failed]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "play_result", %{"track" => Jason.encode!(failed)})
      assert await_backend_prepare_request_count(1) == 1

      send(
        view.pid,
        {:play_prepared, 0, 1, failed, {:error, {:media_registration, "cache unavailable"}}}
      )

      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#player-error", "cache unavailable")
      refute has_element?(view, "#retry-video")
      assert is_nil(:sys.get_state(view.pid).socket.assigns.failed_preparation)
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "an unrelated search error with the same text cannot expose Retry", %{conn: conn} do
    view = live_player(conn, "liveview-preparation-error-context")
    owner = :sys.get_state(view.pid).socket.assigns.owner
    current = track("context-current", "Context current")
    state = snapshot(7, current, owner)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      send(view.pid, {:initial_state, 0, {:ok, state}})
      _ = :sys.get_state(view.pid)

      send(
        view.pid,
        {:prepare_current_result, 0, 7, {:error, {:media_preparation, "media unavailable"}}}
      )

      _ = :sys.get_state(view.pid)

      failure = :sys.get_state(view.pid).socket.assigns.failed_preparation
      assert has_element?(view, "#retry-video")

      view
      |> element("#search-form")
      |> render_submit(%{"search" => %{"query" => "another search"}})

      send(view.pid, {:search_result, 1, {:error, "media unavailable"}})
      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#player-error", "media unavailable")
      refute has_element?(view, "#retry-video")
      assigns = :sys.get_state(view.pid).socket.assigns
      assert assigns.preparation_error? == false
      assert assigns.failed_preparation == failure

      newer_revision = Map.put(state, "revision", 8)
      send(view.pid, {:backend_event, %{"event" => "state_changed", "result" => newer_revision}})
      _ = :sys.get_state(view.pid)

      assert is_nil(:sys.get_state(view.pid).socket.assigns.preparing_current)
      assert :sys.get_state(view.pid).socket.assigns.failed_preparation == failure
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "an empty search clears visible results without contacting the worker", %{conn: conn} do
    conn = init_test_session(conn, %{youtui_session: "liveview-empty-search"})
    {:ok, view, _html} = live(conn, "/")

    view
    |> element("#search-form")
    |> render_submit(%{"search" => %{"query" => "   "}})

    assert has_element?(view, "#search-empty")
    refute has_element?(view, "#search-loading")
  end

  test "an empty successful search renders the no-results state safely", %{conn: conn} do
    view = live_player(conn, "liveview-no-results")

    view
    |> element("#search-form")
    |> render_submit(%{"search" => %{"query" => "demo video"}})

    _ = :sys.get_state(view.pid)

    send(view.pid, {
      :backend_result,
      0,
      {:search_result, 1},
      {:ok, %{"results" => [], "page" => 1, "has_more" => false}}
    })

    _ = :sys.get_state(view.pid)
    assert has_element?(view, "#no-results")
    refute has_element?(view, "#player-error")
  end

  test "search progress from a different session is ignored", %{conn: conn} do
    view = live_player(conn, "liveview-stale-progress")

    send(view.pid, {
      :backend_event,
      %{
        "event" => "search_progress",
        "result" => %{
          "session" => "another-session",
          "generation" => 0,
          "results" => [track("stale-progress")],
          "page" => 1,
          "has_more" => false
        }
      }
    })

    _ = :sys.get_state(view.pid)
    refute has_element?(view, "#result-stale-progress")
  end

  test "foreign playback is labeled and cannot be skipped from this session", %{conn: conn} do
    view = live_player(conn, "liveview-foreign-owner")
    snapshot = snapshot(4, track("remote-video"), "another-session")

    send(view.pid, {:initial_state, 0, {:ok, snapshot}})
    _ = :sys.get_state(view.pid)

    assert has_element?(view, "#play-current-here")
    assert has_element?(view, "#skip-button[disabled]")
    assert has_element?(view, ".yt-other-session", "Playback belongs to another session.")
  end

  test "older snapshots cannot replace the latest state", %{conn: conn} do
    view = live_player(conn, "liveview-old-snapshot")
    latest = snapshot(9, track("current-video", "Latest title"), "another-session")
    stale = snapshot(8, nil, nil)

    send(view.pid, {:initial_state, 0, {:ok, latest}})
    _ = :sys.get_state(view.pid)
    send(view.pid, {:backend_event, %{"event" => "state_changed", "result" => stale}})
    _ = :sys.get_state(view.pid)

    assert has_element?(view, ".yt-now-details strong", "Latest title")
    assert has_element?(view, "#play-current-here")
  end

  test "a later ended snapshot clears the current video", %{conn: conn} do
    view = live_player(conn, "liveview-ended-playback")
    playing = snapshot(6, track("ended-video", "Just ended"), "another-session")
    ended = snapshot(7, nil, nil)

    send(view.pid, {:initial_state, 0, {:ok, playing}})
    _ = :sys.get_state(view.pid)
    assert has_element?(view, ".yt-now-details strong", "Just ended")

    send(view.pid, {:backend_event, %{"event" => "state_changed", "result" => ended}})
    _ = :sys.get_state(view.pid)

    assert has_element?(view, ".yt-now-details strong", "Nothing playing")
    assert has_element?(view, "#skip-button[disabled]")
    refute has_element?(view, "#play-current-here")
  end

  test "worker restart clears old playback and queue state", %{conn: conn} do
    view = live_player(conn, "liveview-worker-restart")

    loaded =
      snapshot(3, track("before-restart"), "another-session")
      |> Map.put("queue", [track("queued-before-restart")])

    send(view.pid, {:initial_state, 0, {:ok, loaded}})
    _ = :sys.get_state(view.pid)
    assert has_element?(view, "#play-current-here")
    assert has_element?(view, "#queue-list")

    send(view.pid, {:backend_event, %{"event" => "disconnected"}})
    _ = :sys.get_state(view.pid)

    refute has_element?(view, "#play-current-here")
    refute has_element?(view, "#queue-list")
    assert has_element?(view, "#queue-empty")
  end

  test "stale preparation completions cannot issue playback commands", %{conn: conn} do
    view = live_player(conn, "liveview-stale-preparation")
    track = track("stale-prepared")

    send(view.pid, {:play_prepared, 0, -1, track, {:ok, "/media/not-issued"}})
    send(view.pid, {:queue_play_prepared, 0, -1, 0, 0, track, {:ok, "/media/not-issued"}})
    _ = :sys.get_state(view.pid)

    refute has_element?(view, "#player-error")
    assert has_element?(view, ".yt-now-details strong", "Nothing playing")
  end

  test "one preparation locks every play control and ignores queued play clicks", %{conn: conn} do
    view = live_player(conn, "liveview-single-preparation")
    first = track("first-video")
    second = track("second-video")
    current = track("foreign-current")

    send(view.pid, {:search_result, 0, {:ok, %{"results" => [first, second]}}})

    send(
      view.pid,
      {:initial_state, 0,
       {:ok, snapshot(2, current, "another-session") |> Map.put("queue", [track("queued")])}}
    )

    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      render_click(view, "play_result", %{"track" => Jason.encode!(first)})
      assert await_backend_prepare_request_count(1) == 1
      assert backend_prepare_request_count() == 1
      assert has_element?(view, "#media-preparing")

      render_click(view, "play_result", %{"track" => Jason.encode!(first)})
      render_click(view, "play_result", %{"track" => Jason.encode!(second)})
      render_click(view, "queue_play", %{"index" => "0", "revision" => "2"})
      render_click(view, "play_current_here", %{})
      _ = :sys.get_state(view.pid)
      settle_backend_tasks()
      assert backend_prepare_request_count() == 1

      assert has_element?(view, "#result-first-video .yt-play-button[disabled]")
      assert has_element?(view, "#result-second-video .yt-play-button[disabled]")
      assert has_element?(view, "#queue-0 .yt-queue-track[disabled]")
      assert has_element?(view, "#play-current-here[disabled]")

      send(view.pid, {:play_prepared, 0, 1, first, {:error, "test preparation failed"}})
      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#player-error", "test preparation failed")
      refute has_element?(view, "#media-preparing")
      refute has_element?(view, "#result-first-video .yt-play-button[disabled]")
      refute has_element?(view, "#result-second-video .yt-play-button[disabled]")
      refute has_element?(view, "#queue-0 .yt-queue-track[disabled]")
      refute has_element?(view, "#play-current-here[disabled]")
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "stale current-media preparation keeps the lock until its completion", %{conn: conn} do
    view = live_player(conn, "liveview-stale-current-preparation")
    owner = :sys.get_state(view.pid).socket.assigns.owner
    result = track("search-result")

    send(view.pid, {:search_result, 0, {:ok, %{"results" => [result]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      send(view.pid, {:initial_state, 0, {:ok, snapshot(4, track("owned-current"), owner)}})
      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#media-preparing")
      assert has_element?(view, "#result-search-result .yt-play-button[disabled]")

      send(
        view.pid,
        {:backend_event,
         %{
           "event" => "state_changed",
           "result" => snapshot(5, track("foreign-current"), "another-session")
         }}
      )

      _ = :sys.get_state(view.pid)
      assert has_element?(view, "#play-current-here[disabled]")
      assert has_element?(view, "#result-search-result .yt-play-button[disabled]")

      send(
        view.pid,
        {:backend_event, %{"event" => "state_changed", "result" => snapshot(6, nil, nil)}}
      )

      _ = :sys.get_state(view.pid)
      assert has_element?(view, "#media-preparing")
      assert has_element?(view, "#result-search-result .yt-play-button[disabled]")

      send(view.pid, {:prepare_current_result, 0, 4, {:ok, "/media/stale.mp4"}})
      _ = :sys.get_state(view.pid)

      refute has_element?(view, "#media-preparing")
      refute has_element?(view, "#result-search-result .yt-play-button[disabled]")
      assert has_element?(view, ".yt-now-details strong", "Nothing playing")

      assigns = :sys.get_state(view.pid).socket.assigns
      assert is_nil(assigns.preparing_current)
      assert is_nil(assigns.loaded_playback_id)
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "manual failure resumes a different deferred current without retrying the failed item", %{
    conn: conn
  } do
    view = live_player(conn, "liveview-deferred-current-after-manual-failure")
    owner = :sys.get_state(view.pid).socket.assigns.owner
    failed = track("manual-video")
    deferred = track("deferred-video", "Deferred video")

    send(view.pid, {:search_result, 0, {:ok, %{"results" => [failed, deferred]}}})
    _ = :sys.get_state(view.pid)
    :ok = :sys.suspend(Youtui.Backend)

    try do
      send(view.pid, {:initial_state, 0, {:ok, snapshot(1, nil, nil)}})
      _ = :sys.get_state(view.pid)

      render_click(view, "play_result", %{"track" => Jason.encode!(failed)})
      assert await_backend_prepare_request_count(1) == 1

      send(
        view.pid,
        {:backend_event, %{"event" => "state_changed", "result" => snapshot(2, deferred, owner)}}
      )

      _ = :sys.get_state(view.pid)
      assert has_element?(view, "#media-preparing")

      send(view.pid, {:play_prepared, 0, 1, failed, {:error, "manual preparation failed"}})
      _ = :sys.get_state(view.pid)

      assert has_element?(view, "#media-preparing")
      assert has_element?(view, ".yt-now-details strong", "Deferred video")
      assert :sys.get_state(view.pid).socket.assigns.preparing_current == 2
      assert await_backend_prepare_request_count(2) == 2

      send(view.pid, {:prepare_current_result, 0, 2, {:error, "deferred preparation failed"}})
      _ = :sys.get_state(view.pid)
      refute has_element?(view, "#media-preparing")

      render_click(view, "play_result", %{"track" => Jason.encode!(deferred)})
      assert await_backend_prepare_request_count(3) == 3
      send(view.pid, {:play_prepared, 0, 2, deferred, {:error, "manual retry failed"}})
      _ = :sys.get_state(view.pid)

      refute has_element?(view, "#media-preparing")
      assert is_nil(:sys.get_state(view.pid).socket.assigns.preparing_current)
      assert has_element?(view, "#player-error", "manual retry failed")
    after
      :ok = :sys.resume(Youtui.Backend)
    end
  end

  test "queue actions reject a revision from an older browser render", %{conn: conn} do
    view = live_player(conn, "liveview-stale-queue-click")
    latest = snapshot(4, nil, nil) |> Map.put("queue", [track("queued-video")])
    send(view.pid, {:initial_state, 0, {:ok, latest}})
    _ = :sys.get_state(view.pid)

    assert has_element?(view, "#queue-0 [phx-value-revision='4']")

    for command <- ["queue_play", "queue_remove"] do
      render_click(view, command, %{"index" => "0", "revision" => "3"})
      assert has_element?(view, "#player-error", "The queue changed.")
      assert has_element?(view, "#queue-0")
      refute has_element?(view, "#media-preparing")
    end
  end

  test "an error from an old browser playback cannot show a new error", %{conn: conn} do
    view = live_player(conn, "liveview-old-video-error")
    render_hook(view, "video_error", %{"playback_id" => -1})
    refute has_element?(view, "#player-error")
  end

  defp live_player(conn, session) do
    conn = init_test_session(conn, %{youtui_session: session})
    {:ok, view, _html} = live(conn, "/")
    _ = :sys.get_state(view.pid)
    view
  end

  defp backend_prepare_request_count do
    {:messages, messages} = Process.info(Process.whereis(Youtui.Backend), :messages)

    Enum.count(messages, fn
      {:"$gen_call", _from, {:request, "prepare_media", _params, _timeout}} -> true
      _ -> false
    end)
  end

  defp backend_setting_requests do
    {:messages, messages} = Process.info(Process.whereis(Youtui.Backend), :messages)

    Enum.flat_map(messages, fn
      {:"$gen_call", _from, {:request, "settings_update", params, _timeout}} -> [params]
      _ -> []
    end)
  end

  defp await_backend_setting_request_count(minimum, attempts \\ 1_000) do
    count = length(backend_setting_requests())

    if count >= minimum or attempts == 0 do
      count
    else
      :erlang.yield()
      await_backend_setting_request_count(minimum, attempts - 1)
    end
  end

  defp await_backend_prepare_request_count(minimum, attempts \\ 1_000) do
    count = backend_prepare_request_count()

    if count >= minimum or attempts == 0 do
      count
    else
      :erlang.yield()
      await_backend_prepare_request_count(minimum, attempts - 1)
    end
  end

  defp settle_backend_tasks do
    Enum.each(1..100, fn _ -> :erlang.yield() end)
  end

  defp snapshot(revision, current, owner) do
    %{
      "revision" => revision,
      "queue" => [],
      "current" => current,
      "playback_id" => if(current, do: revision, else: 0),
      "owner" => owner,
      "settings" => %{
        "include_shorts" => false,
        "sort_by_date" => false,
        "results_per_page" => 20,
        "auto_play_queue" => true
      }
    }
  end

  defp track(id, title \\ "A video") do
    %{
      "id" => id,
      "title" => title,
      "channel" => "A channel",
      "duration" => "1:00",
      "views" => "1K views",
      "published" => ""
    }
  end
end
