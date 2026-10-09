defmodule YoutuiWeb.PlayerLive do
  use YoutuiWeb, :live_view

  alias Youtui.Backend
  alias Youtui.Media

  @backend_timeout 30_000
  @media_timeout 300_000
  @queue_display_limit 200
  @default_settings %{
    "include_shorts" => false,
    "sort_by_date" => false,
    "results_per_page" => 20,
    "auto_play_queue" => true
  }

  @impl true
  def mount(_params, _session, socket) do
    owner = :crypto.strong_rand_bytes(18) |> Base.url_encode64(padding: false)

    socket =
      socket
      |> stream(:results, [], dom_id: &result_dom_id/1)
      |> stream(:queue, [], dom_id: &queue_dom_id/1)
      |> assign(
        page_title: "Youtui",
        owner: owner,
        snapshot: empty_snapshot(),
        result_count: 0,
        query: "",
        search_form: to_form(%{"query" => ""}, as: :search),
        settings_form:
          to_form(%{"results_per_page" => @default_settings["results_per_page"]}, as: :settings),
        search_generation: 0,
        result_tracks: [],
        backend_epoch: 0,
        play_generation: 0,
        search_page: 1,
        has_more: false,
        searching?: false,
        queue_count: 0,
        queue_items: [],
        current: nil,
        playback_id: 0,
        playback_owner: nil,
        settings: @default_settings,
        theater_mode?: false,
        backend_ready?: false,
        preparing_video_id: nil,
        preparing_track: nil,
        preparing_current: nil,
        preparing_current_track: nil,
        loaded_playback_id: nil,
        pending_play: nil,
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )

    if connected?(socket) do
      :ok = Backend.subscribe()
      parent = self()
      epoch = socket.assigns.backend_epoch
      Task.start(fn -> send(parent, {:initial_state, epoch, Backend.request("state")}) end)
    end

    {:ok, socket}
  end

  @impl true
  def handle_event("search", %{"search" => %{"query" => query}}, socket) do
    query = String.trim(query)
    generation = socket.assigns.search_generation + 1

    socket =
      socket
      |> replace_results([])
      |> assign(
        query: query,
        search_form: to_form(%{"query" => query}, as: :search),
        search_generation: generation,
        search_page: 1,
        result_count: 0,
        has_more: false,
        searching?: query != "",
        preparation_error?: false,
        error: nil
      )

    if query == "" do
      {:noreply, cancel_backend_search(socket)}
    else
      params = %{
        "session" => socket.assigns.owner,
        "query" => query,
        "page" => 1,
        "generation" => generation
      }

      {:noreply,
       start_request(socket, {:search_result, generation}, "search", params, @backend_timeout)}
    end
  end

  def handle_event("cancel_search", _params, socket) do
    {:noreply, cancel_backend_search(socket)}
  end

  def handle_event("toggle_theater_mode", _params, socket) do
    {:noreply, assign(socket, :theater_mode?, not socket.assigns.theater_mode?)}
  end

  def handle_event("next_page", _params, %{assigns: %{has_more: true}} = socket) do
    page = socket.assigns.search_page + 1
    generation = socket.assigns.search_generation + 1

    socket =
      assign(socket,
        search_generation: generation,
        search_page: page,
        searching?: true,
        preparation_error?: false,
        error: nil
      )

    params = %{
      "session" => socket.assigns.owner,
      "query" => socket.assigns.query,
      "page" => page,
      "generation" => generation
    }

    {:noreply,
     start_request(socket, {:search_result, generation}, "search", params, @backend_timeout)}
  end

  def handle_event("next_page", _params, socket), do: {:noreply, socket}

  def handle_event("previous_page", _params, %{assigns: %{search_page: page}} = socket)
      when page > 1 do
    page = page - 1
    generation = socket.assigns.search_generation + 1

    socket =
      assign(socket,
        search_generation: generation,
        search_page: page,
        searching?: true,
        preparation_error?: false,
        error: nil
      )

    params = %{
      "session" => socket.assigns.owner,
      "query" => socket.assigns.query,
      "page" => page,
      "generation" => generation
    }

    {:noreply,
     start_request(socket, {:search_result, generation}, "search", params, @backend_timeout)}
  end

  def handle_event("previous_page", _params, socket), do: {:noreply, socket}

  def handle_event("queue_add", %{"track" => encoded_track}, socket) do
    case Jason.decode(encoded_track) do
      {:ok, track} when is_map(track) ->
        {:noreply,
         start_request(
           socket,
           :queue_add_result,
           "queue_add",
           %{"track" => track},
           @backend_timeout
         )}

      _ ->
        {:noreply, assign_error(socket, "This result could not be added to the queue.")}
    end
  end

  def handle_event("play_result", %{"track" => encoded_track}, socket) do
    if preparing_media?(socket.assigns) do
      {:noreply, socket}
    else
      case Jason.decode(encoded_track) do
        {:ok, track} when is_map(track) -> {:noreply, prepare_track(socket, track)}
        _ -> {:noreply, assign_error(socket, "This result could not be played.")}
      end
    end
  end

  def handle_event("play_current_here", _params, %{assigns: %{current: current}} = socket)
      when is_map(current) do
    if preparing_media?(socket.assigns),
      do: {:noreply, socket},
      else: {:noreply, prepare_track(socket, current)}
  end

  def handle_event("play_current_here", _params, socket), do: {:noreply, socket}

  def handle_event(
        "retry_preparation",
        %{"video_id" => video_id, "failure_generation" => failure_generation},
        socket
      ) do
    case socket.assigns.failed_preparation do
      %{track: %{"id" => ^video_id}, generation: generation} = failure ->
        if Integer.to_string(generation) == failure_generation and
             socket.assigns.preparation_error? and socket.assigns.error == failure.error and
             not preparing_media?(socket.assigns) do
          {:noreply, start_retry_task(socket, failure)}
        else
          {:noreply, socket}
        end

      _ ->
        {:noreply, socket}
    end
  end

  def handle_event("retry_preparation", _params, socket), do: {:noreply, socket}

  def handle_event("queue_play", %{"index" => index, "revision" => revision}, socket)
      when is_binary(index) and is_binary(revision) do
    if preparing_media?(socket.assigns) do
      {:noreply, socket}
    else
      with {revision, ""} <- Integer.parse(revision),
           true <- revision == socket.assigns.snapshot["revision"],
           {index, ""} <- Integer.parse(index),
           true <- index >= 0,
           track when is_map(track) <- Enum.at(socket.assigns.snapshot["queue"], index) do
        {:noreply, start_queue_play_task(socket, index, track)}
      else
        _ -> {:noreply, assign_error(socket, "The queue changed. Select the item again.")}
      end
    end
  end

  def handle_event("queue_play", _params, socket), do: {:noreply, socket}

  def handle_event("queue_remove", %{"index" => index, "revision" => revision}, socket)
      when is_binary(index) and is_binary(revision) do
    with {revision, ""} <- Integer.parse(revision),
         true <- revision == socket.assigns.snapshot["revision"],
         {index, ""} <- Integer.parse(index),
         true <- index >= 0 do
      {:noreply,
       start_request(
         socket,
         :queue_result,
         "queue_remove",
         %{"index" => index, "expected_revision" => revision},
         @backend_timeout
       )}
    else
      _ -> {:noreply, assign_error(socket, "The queue changed. Select the item again.")}
    end
  end

  def handle_event("queue_remove", _params, socket), do: {:noreply, socket}

  def handle_event("queue_clear", _params, socket) do
    {:noreply, start_request(socket, :queue_result, "queue_clear", %{}, @backend_timeout)}
  end

  def handle_event("skip", %{"playback_id" => playback_id}, socket)
      when is_binary(playback_id) do
    if preparing_media?(socket.assigns) do
      {:noreply, socket}
    else
      with {playback_id, ""} <- Integer.parse(playback_id), true <- playback_id >= 0 do
        params = %{
          "owner" => socket.assigns.owner,
          "playback_id" => playback_id
        }

        {:noreply, start_request(socket, :playback_result, "skip", params, @backend_timeout)}
      else
        _ -> {:noreply, socket}
      end
    end
  end

  def handle_event("skip", _params, socket), do: {:noreply, socket}

  def handle_event("setting", %{"field" => field, "enabled" => value}, socket) do
    case setting_value(field, value) do
      {:ok, parsed} ->
        params = %{field => parsed}

        {:noreply,
         start_request(socket, :settings_result, "settings_update", params, @backend_timeout)}

      :error ->
        {:noreply, assign_error(socket, "That setting is not supported.")}
    end
  end

  def handle_event("setting", %{"settings" => %{"results_per_page" => value}}, socket) do
    case Integer.parse(value) do
      {count, ""} when count >= 1 and count <= 500 ->
        {:noreply,
         start_request(
           socket,
           :settings_result,
           "settings_update",
           %{"results_per_page" => count},
           @backend_timeout
         )}

      _ ->
        {:noreply, assign_error(socket, "Results per page must be between 1 and 500.")}
    end
  end

  def handle_event("video_ended", %{"playback_id" => playback_id}, socket) do
    with {playback_id, ""} <- Integer.parse(to_string(playback_id)) do
      params = %{"owner" => socket.assigns.owner, "playback_id" => playback_id}

      {:noreply,
       start_request(socket, :playback_result, "playback_ended", params, @backend_timeout)}
    else
      _ -> {:noreply, socket}
    end
  end

  def handle_event("video_error", %{"playback_id" => playback_id}, socket) do
    if playback_id == socket.assigns.playback_id and
         socket.assigns.playback_owner == socket.assigns.owner do
      {:noreply, assign_error(socket, "This video could not be played. Try preparing it again.")}
    else
      {:noreply, socket}
    end
  end

  def handle_event("video_error", _params, socket), do: {:noreply, socket}

  @impl true
  def handle_info({:initial_state, epoch, {:ok, snapshot}}, socket) do
    if epoch == socket.assigns.backend_epoch do
      {:noreply, apply_snapshot(socket, snapshot, true)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:initial_state, epoch, {:error, _message}}, socket) do
    if epoch == socket.assigns.backend_epoch do
      {:noreply, assign(socket, backend_ready?: false)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:search_result, generation, {:ok, result}}, socket) do
    if generation == socket.assigns.search_generation do
      {:noreply, apply_search_result(socket, result)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:search_result, generation, {:error, message}}, socket) do
    if generation == socket.assigns.search_generation do
      {:noreply, socket |> assign(searching?: false) |> assign_error(message)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:backend_result, epoch, tag, result}, socket) do
    if epoch == socket.assigns.backend_epoch do
      {:noreply, handle_backend_result(socket, tag, result)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:play_prepared, epoch, generation, track, result}, socket) do
    if epoch == socket.assigns.backend_epoch and generation == socket.assigns.play_generation and
         socket.assigns.preparing_video_id == track["id"] do
      {:noreply, commit_prepared_play(socket, track, result)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:retry_prepared, epoch, generation, failure, snapshot, result}, socket) do
    if epoch == socket.assigns.backend_epoch and generation == socket.assigns.play_generation and
         socket.assigns.preparing_video_id == failure.track["id"] do
      cond do
        is_map(snapshot) and retry_snapshot_matches?(snapshot, failure) and
            retry_snapshot_matches?(socket.assigns.snapshot, failure) ->
          {:noreply, commit_retry_result(socket, failure, snapshot, result)}

        is_map(snapshot) ->
          {:noreply, clear_stale_retry(socket, snapshot)}

        true ->
          {:noreply, clear_pending_play(socket, "Playback changed. Select this video again.")}
      end
    else
      {:noreply, socket}
    end
  end

  def handle_info({:retry_stale, epoch, generation, snapshot}, socket) do
    if epoch == socket.assigns.backend_epoch and generation == socket.assigns.play_generation do
      {:noreply, clear_stale_retry(socket, snapshot)}
    else
      {:noreply, socket}
    end
  end

  def handle_info(
        {:queue_play_prepared, epoch, generation, index, expected_revision, track, result},
        socket
      ) do
    if epoch == socket.assigns.backend_epoch and generation == socket.assigns.play_generation and
         socket.assigns.preparing_video_id == track["id"] do
      if expected_revision == socket.assigns.snapshot["revision"] do
        {:noreply, commit_prepared_queue_play(socket, index, expected_revision, track, result)}
      else
        {:noreply,
         clear_pending_play(socket, "The queue changed while that video was preparing.")}
      end
    else
      {:noreply, socket}
    end
  end

  def handle_info({:prepare_current_result, epoch, playback_id, result}, socket) do
    if epoch == socket.assigns.backend_epoch and socket.assigns.preparing_current == playback_id do
      track = socket.assigns.preparing_current_track
      socket = assign(socket, preparing_current: nil, preparing_current_track: nil)

      if current_playback?(socket.assigns, playback_id) do
        case result do
          {:ok, url} ->
            socket =
              socket
              |> assign(loaded_playback_id: playback_id, error: nil, preparation_error?: false)
              |> refresh_play_controls()
              |> push_event("load_video", %{url: url, playback_id: playback_id})

            {:noreply, socket}

          {:error, {:media_preparation, message}} ->
            {:noreply, mark_failed_current_preparation(socket, track, message)}

          {:error, {:media_registration, message}} ->
            {:noreply,
             socket
             |> assign_error(safe_error_detail(message))
             |> refresh_play_controls()}

          {:error, {:retry_state, _message}} ->
            {:noreply,
             socket
             |> assign_error("Playback changed. Select this video again.")
             |> refresh_play_controls()}

          {:error, message} ->
            {:noreply,
             socket
             |> assign_error(safe_error_detail(message))
             |> refresh_play_controls()}
        end
      else
        {:noreply,
         socket
         |> refresh_play_controls()
         |> prepare_deferred_current(playback_id)}
      end
    else
      {:noreply, socket}
    end
  end

  def handle_info({:backend_event, %{"event" => "disconnected"}}, socket) do
    socket = reset_worker_state(socket)
    {:noreply, push_event(socket, "stop_video", %{})}
  end

  def handle_info({:backend_event, %{"event" => "ready"}}, socket) do
    socket = reset_worker_state(socket)
    epoch = socket.assigns.backend_epoch
    parent = self()
    Task.start(fn -> send(parent, {:initial_state, epoch, Backend.request("state")}) end)
    {:noreply, assign(socket, :backend_ready?, true)}
  end

  def handle_info({:backend_event, %{"event" => "state_changed", "result" => snapshot}}, socket) do
    if socket.assigns.backend_ready? do
      {:noreply, apply_snapshot(socket, snapshot, false)}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:backend_event, %{"event" => "search_progress", "result" => result}}, socket) do
    if result["session"] == socket.assigns.owner and
         result["generation"] == socket.assigns.search_generation do
      {:noreply,
       socket
       |> replace_results(result["results"] || [])
       |> assign(
         search_page: result["page"] || socket.assigns.search_page,
         has_more: result["has_more"] || false
       )}
    else
      {:noreply, socket}
    end
  end

  def handle_info({:backend_event, _event}, socket), do: {:noreply, socket}

  @impl true
  def render(assigns) do
    ~H"""
    <Layouts.app flash={@flash}>
      <div class="youtui-shell">
        <header class="yt-topbar">
          <a class="yt-brand" href="/" aria-label="Youtui home">
            <span class="yt-brand-mark"><.icon name="hero-play-solid" class="size-4" /></span>
            <span>youtui</span>
          </a>
          <div class="yt-topbar-right">
            <span class={[@backend_ready? && "is-ready", !@backend_ready? && "is-offline"]}>
              <span class="yt-status-dot"></span>
              {if @backend_ready?, do: "Connected", else: "Starting worker"}
            </span>
            <span class="yt-local-label"><.icon name="hero-computer-desktop" class="size-4" />
            Local session</span>
          </div>
        </header>

        <main class="yt-main">
          <h1 id="workspace-title" class="sr-only">youtui</h1>

          <section class="yt-search-panel" aria-label="Search videos">
            <.form for={@search_form} id="search-form" phx-submit="search">
              <.input
                field={@search_form[:query]}
                id="search-query"
                type="search"
                placeholder="Search songs, creators, and more"
                autocomplete="off"
                class="yt-search-input"
                aria-label="Search YouTube videos"
              />
              <button
                class="yt-search-button"
                type="submit"
                aria-label="Search YouTube videos"
                disabled={@searching?}
              >
                <.icon name="hero-magnifying-glass" class="size-5" />
                <span>{if @searching?, do: "Searching", else: "Search"}</span>
              </button>
            </.form>
            <div class="yt-search-meta">
              <span :if={@query != ""} class="yt-query-label">Results for <strong>{@query}</strong></span>
              <span class="yt-shortcut-note">Search YouTube</span>
              <span class="yt-key-hint"><kbd>Enter</kbd><span>to search</span></span>
              <button :if={@searching?} id="cancel-search" type="button" phx-click="cancel_search">Cancel</button>
            </div>
          </section>

          <div :if={@error} id="player-error" class="yt-error" role="alert">
            <.icon name="hero-exclamation-circle" class="size-5 shrink-0" />
            <div class="yt-error-content">
              <strong class="yt-error-title">
                {if @preparation_error?,
                  do: "Video preparation failed: #{@failed_preparation.track["title"]}",
                  else: "Action could not be completed"}
              </strong>
              <p class="yt-error-message">{@error}</p>
              <p
                :if={@preparation_error?}
                class="yt-error-message"
              >
                Select Retry to prepare this video again.
              </p>
              <button
                :if={@preparation_error? && @failed_preparation}
                id="retry-video"
                class="yt-retry-button"
                type="button"
                phx-click="retry_preparation"
                phx-value-video_id={@failed_preparation.track["id"]}
                phx-value-failure_generation={@failed_preparation.generation}
                disabled={not is_nil(@preparing_video_id) || not is_nil(@preparing_current)}
              >Retry</button>
            </div>
          </div>

          <section class={["yt-workspace", @theater_mode? && "yt-workspace--theater"]}>
            <section id="player-card" class="yt-player-card" aria-label="Video player">
              <div class="yt-card-heading">
                <div>
                  <h2>Video player</h2>
                </div>
                <div class="yt-player-heading-actions">
                  <span class="yt-live-badge"><span></span> LOCAL</span>
                  <button
                    id="theater-toggle"
                    class="yt-theater-button"
                    type="button"
                    phx-click="toggle_theater_mode"
                    aria-pressed={to_string(@theater_mode?)}
                    title={if @theater_mode?, do: "Exit theater mode", else: "Enter theater mode"}
                  >
                    <.icon name="hero-arrows-right-left" class="size-4" /> Theater mode
                  </button>
                </div>
              </div>
              <div
                id="player-surface"
                class="yt-player-surface"
                phx-hook="YoutuiPlayer"
                phx-update="ignore"
              >
                <video
                  id="player-video"
                  class="yt-video"
                  controls
                  playsinline
                  preload="metadata"
                  aria-label="Video player"
                ></video>
                <div id="player-placeholder" class="yt-player-placeholder">
                  <span class="yt-player-glow"></span>
                  <span class="yt-player-logo"><.icon name="hero-play-solid" class="size-6" /></span>
                  <strong>Your next favorite goes here</strong>
                  <span>Choose a result to prepare it for playback</span>
                </div>
                <button id="player-manual-play" class="yt-manual-play" type="button" hidden>
                  <.icon name="hero-play-solid" class="size-4" /> Play video
                </button>
              </div>
              <div
                :if={@preparing_video_id || @preparing_current}
                id="media-preparing"
                class="yt-preparing-message"
                role="status"
              >
                <strong class="yt-preparing-message-title">
                  {(@preparing_track && @preparing_track["title"]) ||
                    (@preparing_current_track && @preparing_current_track["title"]) ||
                    "Selected video"}
                </strong>
                <span class="yt-preparing-message-detail">
                  Preparing this video for playback. Other videos wait until this finishes.
                </span>
              </div>
              <div class="yt-now-playing">
                <div class="yt-now-art"><.icon name="hero-video-camera" class="size-5" /></div>
                <div class="yt-now-details">
                  <strong>{if @current, do: @current["title"], else: "Nothing playing"}</strong>
                  <span>{if @current, do: @current["channel"], else: "Pick a video to get started"}</span>
                </div>
                <button
                  id="skip-button"
                  class="yt-icon-button"
                  type="button"
                  aria-label={
                    if @current, do: "Skip #{@current["title"]}", else: "Skip current video"
                  }
                  phx-click="skip"
                  phx-value-playback_id={@playback_id}
                  disabled={
                    is_nil(@current) || @playback_owner != @owner ||
                      not is_nil(@preparing_video_id) || not is_nil(@preparing_current)
                  }
                ><.icon name="hero-forward-solid" class="size-5" /></button>
              </div>
              <div :if={@current && @playback_owner != @owner} class="yt-other-session">
                <span>Playback belongs to another session.</span>
                <button
                  id="play-current-here"
                  type="button"
                  aria-label={"Play #{@current["title"]} in this session"}
                  phx-click="play_current_here"
                  disabled={not is_nil(@preparing_video_id) || not is_nil(@preparing_current)}
                >
                  Play here
                </button>
              </div>
            </section>

            <div class="yt-results-column">
              <div class="yt-section-heading">
                <div>
                  <h2>{if @query == "", do: "Search to get started", else: "Search results"}</h2>
                </div>
                <span :if={@result_count > 0} class="yt-result-count">{@result_count} shown</span>
              </div>

              <div :if={@searching? && @result_count == 0} id="search-loading" class="yt-loading-card">
                <span class="yt-spinner"></span>
                <div>
                  <strong>Finding videos</strong><p>Good things take a moment.</p>
                </div>
              </div>
              <div :if={@query == "" && @result_count == 0} id="search-empty" class="yt-empty-card">
                <div class="yt-empty-icon"><.icon name="hero-sparkles" class="size-6" /></div>
                <strong>Start with a search</strong>
                <p>Try an artist, album, live set, or something you want to learn.</p>
              </div>
              <div
                :if={@query != "" && not @searching? && @result_count == 0 && is_nil(@error)}
                id="no-results"
                class="yt-empty-card"
              >
                <div class="yt-empty-icon"><.icon name="hero-magnifying-glass" class="size-6" /></div>
                <strong>No videos found yet</strong>
                <p>Try a shorter phrase or a different spelling.</p>
              </div>

              <div
                :if={@result_count > 0}
                id="search-results"
                class="yt-results-list"
                phx-update="stream"
              >
                <article :for={{dom_id, track} <- @streams.results} id={dom_id} class="yt-result-card">
                  <div class="yt-result-art">
                    <span><.icon name="hero-play" class="size-5" /></span>
                  </div>
                  <div class="yt-result-details">
                    <h3>{track["title"]}</h3>
                    <p class="yt-result-channel">{track["channel"]}</p>
                    <div class="yt-track-meta">
                      <span>{track["duration"]}</span><span>{track["views"]}</span><span :if={
                        track["published"] != ""
                      }>{track["published"]}</span>
                    </div>
                  </div>
                  <div class="yt-result-actions">
                    <button
                      class="yt-icon-button yt-add-button"
                      type="button"
                      aria-label={"Add #{track["title"]} to queue"}
                      title={"Add #{track["title"]} to queue"}
                      phx-click="queue_add"
                      phx-value-track={Jason.encode!(track)}
                    ><.icon name="hero-plus" class="size-5" /></button>
                    <button
                      class="yt-play-button"
                      type="button"
                      aria-label={
                        if @preparing_video_id == track["id"],
                          do: "Preparing #{track["title"]}",
                          else: "Play #{track["title"]}"
                      }
                      phx-click="play_result"
                      phx-value-track={Jason.encode!(track)}
                      disabled={not is_nil(@preparing_video_id) || not is_nil(@preparing_current)}
                    >
                      <.icon
                        :if={@preparing_video_id != track["id"]}
                        name="hero-play-solid"
                        class="size-4"
                      />
                      <span :if={@preparing_video_id == track["id"]} class="yt-small-spinner"></span>
                      {if @preparing_video_id == track["id"], do: "Preparing", else: "Play"}
                    </button>
                  </div>
                </article>
              </div>

              <div :if={@query != ""} class="yt-pagination">
                <button
                  id="previous-page"
                  type="button"
                  phx-click="previous_page"
                  disabled={@search_page <= 1 || @searching?}
                >
                  <.icon name="hero-arrow-left" class="size-4" /> Previous
                </button>
                <span>Page {@search_page}</span>
                <button
                  id="next-page"
                  type="button"
                  phx-click="next_page"
                  disabled={!@has_more || @searching?}
                >
                  Next <.icon name="hero-arrow-right" class="size-4" />
                </button>
              </div>
            </div>

            <aside class="yt-side-column">
              <section class="yt-queue-card" aria-label="Up next queue">
                <div class="yt-card-heading">
                  <div>
                    <h2>
                      Up next <span class="yt-count-pill">{@queue_count}</span>
                    </h2>
                  </div>
                  <button
                    id="clear-queue"
                    class="yt-text-button"
                    type="button"
                    aria-label="Clear queue"
                    phx-click="queue_clear"
                    disabled={@queue_count == 0}
                  >Clear</button>
                </div>
                <div :if={@queue_count == 0} id="queue-empty" class="yt-queue-empty">
                  <.icon name="hero-queue-list" class="size-6" />
                  <span>Your queue is open.</span>
                  <span>Add videos with the plus button.</span>
                </div>
                <ol :if={@queue_count > 0} id="queue-list" class="yt-queue-list" phx-update="stream">
                  <li
                    :for={{dom_id, item} <- @streams.queue}
                    id={dom_id}
                    class="yt-queue-item"
                  >
                    <span class="yt-queue-index">{String.pad_leading(
                      Integer.to_string(item.index + 1),
                      2,
                      "0"
                    )}</span>
                    <button
                      class="yt-queue-track"
                      type="button"
                      aria-label={"Play #{item.track["title"]} from queue"}
                      phx-click="queue_play"
                      phx-value-index={item.index}
                      phx-value-revision={@snapshot["revision"]}
                      disabled={not is_nil(@preparing_video_id) || not is_nil(@preparing_current)}
                    >
                      <strong>{item.track["title"]}</strong><span>{item.track["channel"]} · {item.track[
                        "duration"
                      ]}</span>
                    </button>
                    <button
                      class="yt-icon-button yt-remove-button"
                      type="button"
                      aria-label={"Remove #{item.track["title"]} from queue"}
                      phx-click="queue_remove"
                      phx-value-index={item.index}
                      phx-value-revision={@snapshot["revision"]}
                    >
                      <.icon name="hero-x-mark" class="size-4" />
                    </button>
                  </li>
                </ol>
              </section>

              <details class="yt-settings-card">
                <summary id="settings-toggle">
                  <span><.icon name="hero-adjustments-horizontal" class="size-5" /> Playback settings</span><.icon
                    name="hero-chevron-down"
                    class="size-4"
                  />
                </summary>
                <div class="yt-settings-body">
                  <button
                    id="include-shorts-toggle"
                    type="button"
                    class="yt-setting-row"
                    role="switch"
                    aria-label="Include Shorts"
                    aria-checked={to_string(@settings["include_shorts"])}
                    phx-click="setting"
                    phx-value-field="include_shorts"
                    phx-value-enabled={to_string(not @settings["include_shorts"])}
                  >
                    <span><strong>Include Shorts</strong><small>Show vertical videos in search</small></span>
                    <span class={[@settings["include_shorts"] && "yt-switch-on", "yt-switch"]}><span></span></span>
                  </button>
                  <button
                    id="sort-by-newest-toggle"
                    type="button"
                    class="yt-setting-row"
                    role="switch"
                    aria-label="Sort by newest"
                    aria-checked={to_string(@settings["sort_by_date"])}
                    phx-click="setting"
                    phx-value-field="sort_by_date"
                    phx-value-enabled={to_string(not @settings["sort_by_date"])}
                  >
                    <span><strong>Sort by newest</strong><small>Prioritize recent uploads</small></span>
                    <span class={[@settings["sort_by_date"] && "yt-switch-on", "yt-switch"]}><span></span></span>
                  </button>
                  <button
                    id="autoplay-queue-toggle"
                    type="button"
                    class="yt-setting-row"
                    role="switch"
                    aria-label="Autoplay queue"
                    aria-checked={to_string(@settings["auto_play_queue"])}
                    phx-click="setting"
                    phx-value-field="auto_play_queue"
                    phx-value-enabled={to_string(not @settings["auto_play_queue"])}
                  >
                    <span><strong>Autoplay queue</strong><small>Continue when a video ends</small></span>
                    <span class={[@settings["auto_play_queue"] && "yt-switch-on", "yt-switch"]}><span></span></span>
                  </button>
                  <.form for={@settings_form} id="results-per-page-form" phx-change="setting">
                    <label for="results-per-page">Results per page</label>
                    <select id="results-per-page" name="settings[results_per_page]">
                      <option
                        :for={count <- [10, 20, 30, 50]}
                        value={count}
                        selected={count == @settings["results_per_page"]}
                      >
                        {count} videos
                      </option>
                    </select>
                  </.form>
                </div>
              </details>
            </aside>
          </section>

          <footer class="yt-footer">
            <span>Youtui</span><span>Private by design · Runs on your machine</span>
          </footer>
        </main>
      </div>
    </Layouts.app>
    """
  end

  defp start_request(socket, tag, command, params, timeout) do
    socket = assign(socket, error: nil, preparation_error?: false)
    parent = self()
    epoch = socket.assigns.backend_epoch

    Task.start(fn ->
      send(parent, {:backend_result, epoch, tag, Backend.request(command, params, timeout)})
    end)

    socket
  end

  defp start_queue_play_task(socket, index, track) do
    parent = self()
    epoch = socket.assigns.backend_epoch
    generation = socket.assigns.play_generation + 1
    expected_revision = socket.assigns.snapshot["revision"]

    socket =
      assign(socket,
        play_generation: generation,
        preparing_video_id: track["id"],
        preparing_track: track,
        pending_play: %{
          kind: :queue,
          generation: generation,
          track_id: track["id"],
          track: track,
          index: index,
          expected_revision: expected_revision
        },
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )
      |> refresh_play_controls()

    Task.start(fn ->
      result = prepare_media(track["id"])

      send(
        parent,
        {:queue_play_prepared, epoch, generation, index, expected_revision, track, result}
      )
    end)

    socket
  end

  defp start_retry_task(socket, failure) do
    parent = self()
    epoch = socket.assigns.backend_epoch
    generation = socket.assigns.play_generation + 1
    track = failure.track

    socket =
      assign(socket,
        play_generation: generation,
        preparing_video_id: track["id"],
        preparing_track: track,
        pending_play: Map.merge(failure, %{generation: generation}),
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )
      |> refresh_play_controls()

    Task.start(fn ->
      case Backend.request("state", %{}, @backend_timeout) do
        {:ok, snapshot} ->
          if retry_snapshot_matches?(snapshot, failure) do
            result = prepare_media(track["id"])

            case Backend.request("state", %{}, @backend_timeout) do
              {:ok, prepared_snapshot} ->
                send(
                  parent,
                  {:retry_prepared, epoch, generation, failure, prepared_snapshot, result}
                )

              {:error, message} ->
                send(
                  parent,
                  {:retry_prepared, epoch, generation, failure, nil,
                   {:error, {:retry_state, message}}}
                )
            end
          else
            send(parent, {:retry_stale, epoch, generation, snapshot})
          end

        {:error, message} ->
          send(
            parent,
            {:retry_prepared, epoch, generation, failure, nil, {:error, {:retry_state, message}}}
          )
      end
    end)

    socket
  end

  defp commit_retry_result(socket, %{kind: :queue} = failure, _snapshot, result) do
    commit_prepared_queue_play(
      socket,
      failure.index,
      failure.expected_revision,
      failure.track,
      result
    )
  end

  defp commit_retry_result(socket, %{kind: :current} = failure, snapshot, {:ok, url}) do
    playback_id = snapshot["playback_id"]

    if get_in(snapshot, ["current", "id"]) == failure.track["id"] and
         snapshot["owner"] == socket.assigns.owner do
      socket
      |> assign(
        loaded_playback_id: playback_id,
        preparing_video_id: nil,
        preparing_track: nil,
        pending_play: nil,
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )
      |> refresh_play_controls()
      |> push_event("load_video", %{url: url, playback_id: playback_id})
    else
      clear_stale_retry(socket, snapshot)
    end
  end

  defp commit_retry_result(socket, failure, _snapshot, result),
    do: commit_prepared_play(socket, failure.track, result)

  defp prepare_track(socket, track) do
    parent = self()
    epoch = socket.assigns.backend_epoch
    generation = socket.assigns.play_generation + 1

    socket =
      assign(socket,
        play_generation: generation,
        preparing_video_id: track["id"],
        preparing_track: track,
        pending_play: %{
          kind: :track,
          generation: generation,
          track_id: track["id"],
          track: track
        },
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )
      |> refresh_play_controls()

    Task.start(fn ->
      result = prepare_media(track["id"])
      send(parent, {:play_prepared, epoch, generation, track, result})
    end)

    socket
  end

  defp prepare_media(video_id) do
    case Backend.request("prepare_media", %{"video_id" => video_id}, @media_timeout) do
      {:ok, media} ->
        case Media.register(media) do
          {:ok, url} -> {:ok, url}
          {:error, message} -> {:error, {:media_registration, message}}
        end

      {:error, message} ->
        {:error, {:media_preparation, message}}
    end
  end

  defp commit_prepared_play(socket, track, {:ok, url}) do
    case Backend.request(
           "play",
           %{"track" => track, "owner" => socket.assigns.owner},
           @backend_timeout
         ) do
      {:ok, snapshot} -> install_prepared_playback(socket, track, snapshot, url)
      {:error, message} -> clear_pending_play(socket, message)
    end
  end

  defp commit_prepared_play(socket, _track, {:error, {:media_preparation, message}}),
    do: clear_pending_play(socket, message, true)

  defp commit_prepared_play(socket, _track, {:error, {:media_registration, message}}),
    do: clear_pending_play(socket, message)

  defp commit_prepared_play(socket, _track, {:error, {:retry_state, _message}}),
    do: clear_pending_play(socket, "Playback changed. Select this video again.")

  defp commit_prepared_play(socket, _track, {:error, message}),
    do: clear_pending_play(socket, message, true)

  defp commit_prepared_queue_play(socket, index, expected_revision, track, {:ok, url}) do
    case Backend.request(
           "queue_play",
           %{
             "index" => index,
             "owner" => socket.assigns.owner,
             "expected_revision" => expected_revision
           },
           @backend_timeout
         ) do
      {:ok, snapshot} -> install_prepared_playback(socket, track, snapshot, url)
      {:error, message} -> clear_pending_play(socket, message)
    end
  end

  defp commit_prepared_queue_play(
         socket,
         _index,
         _expected_revision,
         _track,
         {:error, {:media_preparation, message}}
       ),
       do: clear_pending_play(socket, message, true)

  defp commit_prepared_queue_play(
         socket,
         _index,
         _expected_revision,
         _track,
         {:error, {:media_registration, message}}
       ),
       do: clear_pending_play(socket, message)

  defp commit_prepared_queue_play(
         socket,
         _index,
         _expected_revision,
         _track,
         {:error, {:retry_state, _message}}
       ),
       do: clear_pending_play(socket, "Playback changed. Select this video again.")

  defp commit_prepared_queue_play(socket, _index, _expected_revision, _track, {:error, message}),
    do: clear_pending_play(socket, message, true)

  defp install_prepared_playback(socket, track, snapshot, url) do
    playback_id = snapshot["playback_id"]

    owns_current? =
      snapshot["owner"] == socket.assigns.owner and
        get_in(snapshot, ["current", "id"]) == track["id"]

    current_enough? = (snapshot["revision"] || -1) >= socket.assigns.snapshot["revision"]

    if owns_current? and current_enough? do
      socket
      |> apply_snapshot(snapshot, false, true)
      |> assign(
        loaded_playback_id: playback_id,
        preparing_video_id: nil,
        preparing_track: nil,
        pending_play: nil,
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )
      |> refresh_play_controls()
      |> push_event("load_video", %{url: url, playback_id: playback_id})
    else
      clear_pending_play(socket, "Playback changed before this video was ready.")
    end
  end

  defp clear_pending_play(socket, message, retryable? \\ false) do
    pending = socket.assigns.pending_play
    failed_video_id = get_in(pending, [:track_id])
    detail = safe_error_detail(message)

    failed_preparation =
      if retryable? and is_map(pending) and is_map(pending[:track]) do
        pending
        |> Map.merge(%{
          generation: socket.assigns.play_generation,
          owner: socket.assigns.owner,
          backend_epoch: socket.assigns.backend_epoch,
          revision: socket.assigns.snapshot["revision"],
          playback_id: socket.assigns.playback_id,
          playback_owner: socket.assigns.playback_owner,
          error: detail
        })
      else
        nil
      end

    socket
    |> assign(
      preparing_video_id: nil,
      preparing_track: nil,
      pending_play: nil,
      failed_preparation: failed_preparation,
      preparation_error?: not is_nil(failed_preparation),
      error: detail
    )
    |> refresh_play_controls()
    |> prepare_deferred_current(nil, failed_video_id)
  end

  defp safe_error_detail(message) do
    message
    |> to_string()
    |> String.replace(~r/[\x00-\x1F\x7F]/u, " ")
    |> String.trim()
    |> String.slice(0, 400)
    |> case do
      "" -> "The video could not be prepared."
      detail -> detail
    end
  end

  defp assign_error(socket, message) do
    assign(socket, error: message, preparation_error?: false)
  end

  defp mark_failed_current_preparation(socket, track, message) do
    detail = safe_error_detail(message)

    failure = %{
      kind: :current,
      track: track,
      generation: socket.assigns.play_generation,
      owner: socket.assigns.owner,
      backend_epoch: socket.assigns.backend_epoch,
      revision: socket.assigns.snapshot["revision"],
      playback_id: socket.assigns.playback_id,
      playback_owner: socket.assigns.playback_owner,
      error: detail
    }

    socket
    |> assign(failed_preparation: failure, preparation_error?: true, error: detail)
    |> refresh_play_controls()
  end

  defp start_current_media(socket, track, playback_id) do
    socket =
      socket
      |> assign(
        preparing_current: playback_id,
        preparing_current_track: track,
        failed_preparation: nil,
        preparation_error?: false,
        error: nil
      )
      |> refresh_play_controls()

    parent = self()
    owner = socket.assigns.owner
    epoch = socket.assigns.backend_epoch

    Task.start(fn ->
      result =
        case Backend.request(
               "prepare_media",
               %{"video_id" => track["id"]},
               @media_timeout
             ) do
          {:ok, media} ->
            case Media.register(media) do
              {:ok, url} ->
                case Backend.request("state", %{}, @backend_timeout) do
                  {:ok, snapshot} ->
                    if snapshot["owner"] == owner and snapshot["playback_id"] == playback_id do
                      {:ok, url}
                    else
                      {:error, {:retry_state, "Playback changed while the video was preparing."}}
                    end

                  {:error, message} ->
                    {:error, {:retry_state, message}}
                end

              {:error, message} ->
                {:error, {:media_registration, message}}
            end

          {:error, message} ->
            {:error, {:media_preparation, message}}
        end

      send(parent, {:prepare_current_result, epoch, playback_id, result})
    end)

    socket
  end

  defp apply_snapshot(socket, snapshot, _restoring?, media_prepared? \\ false) do
    revision = snapshot["revision"] || 0

    if revision < socket.assigns.snapshot["revision"] do
      socket
    else
      current = snapshot["current"]
      playback_id = snapshot["playback_id"] || 0
      owner = snapshot["owner"]
      previous_current = socket.assigns.current
      changed_owner? = owner != socket.assigns.playback_owner

      socket =
        if changed_owner? do
          assign(socket, :loaded_playback_id, nil)
        else
          socket
        end

      socket =
        socket
        |> replace_queue(snapshot["queue"] || [])
        |> assign(
          snapshot: snapshot,
          queue_count: length(snapshot["queue"] || []),
          current: current,
          playback_id: playback_id,
          playback_owner: owner,
          settings: snapshot["settings"] || @default_settings,
          settings_form:
            to_form(
              %{
                "results_per_page" =>
                  get_in(snapshot, ["settings", "results_per_page"]) ||
                    @default_settings["results_per_page"]
              },
              as: :settings
            ),
          backend_ready?: true
        )
        |> clear_stale_failed_preparation(snapshot)

      cond do
        changed_owner? && owner != socket.assigns.owner ->
          push_event(socket, "stop_video", %{})

        current && owner == socket.assigns.owner && not media_prepared? &&
          not preparing_media?(socket.assigns) &&
          socket.assigns.loaded_playback_id != playback_id &&
            not failed_current_preparation?(socket.assigns, current, playback_id) ->
          start_current_media(socket, current, playback_id)

        current == nil &&
            (previous_current != nil || socket.assigns.loaded_playback_id != nil) ->
          socket
          |> assign(:loaded_playback_id, nil)
          |> push_event("stop_video", %{})

        true ->
          socket
      end
      |> refresh_play_controls()
    end
  end

  defp apply_search_result(socket, result) do
    socket
    |> replace_results(result["results"] || [])
    |> assign(
      search_page: result["page"] || socket.assigns.search_page,
      has_more: result["has_more"] || false,
      searching?: false,
      preparation_error?: false,
      error: nil
    )
  end

  defp replace_results(socket, results) do
    results = Enum.take(results, 500)

    socket
    |> stream(:results, results, reset: true)
    |> assign(:result_tracks, results)
    |> assign(:result_count, length(results))
  end

  defp replace_queue(socket, queue) do
    items =
      queue
      |> Enum.take(@queue_display_limit)
      |> Enum.with_index()
      |> Enum.map(fn {track, index} ->
        %{stream_id: "queue-#{index}", track: track, index: index}
      end)

    socket
    |> stream(:queue, items, reset: true)
    |> assign(:queue_items, items)
    |> assign(:queue_count, length(queue))
  end

  defp refresh_play_controls(socket) do
    socket
    |> stream(:results, socket.assigns.result_tracks, reset: true)
    |> stream(:queue, socket.assigns.queue_items, reset: true)
  end

  defp preparing_media?(assigns) do
    not is_nil(assigns.preparing_video_id) or not is_nil(assigns.preparing_current)
  end

  defp current_playback?(assigns, playback_id) do
    is_map(assigns.current) and assigns.playback_id == playback_id and
      assigns.playback_owner == assigns.owner
  end

  defp retry_snapshot_matches?(snapshot, failure) do
    same_playback? =
      (failure.kind == :current or snapshot["revision"] == failure.revision) and
        snapshot["playback_id"] == failure.playback_id and
        snapshot["owner"] == failure.playback_owner

    same_queue_item? =
      if failure.kind == :queue do
        failure.expected_revision == snapshot["revision"] and
          get_in(snapshot, ["queue", Access.at(failure.index), "id"]) == failure.track["id"]
      else
        true
      end

    same_current? =
      if failure.kind == :current do
        get_in(snapshot, ["current", "id"]) == failure.track["id"]
      else
        true
      end

    same_playback? and same_queue_item? and same_current?
  end

  defp failed_current_preparation?(assigns, current, playback_id) do
    case assigns.failed_preparation do
      %{kind: :current} = failure ->
        failure.backend_epoch == assigns.backend_epoch and
          failure.playback_id == playback_id and
          failure.playback_owner == assigns.owner and
          failure.track["id"] == current["id"]

      _ ->
        false
    end
  end

  defp clear_stale_failed_preparation(socket, snapshot) do
    case socket.assigns.failed_preparation do
      nil ->
        socket

      failure ->
        if retry_snapshot_matches?(snapshot, failure) and
             failure.owner == socket.assigns.owner and
             failure.backend_epoch == socket.assigns.backend_epoch do
          socket
        else
          assign(socket,
            failed_preparation: nil,
            preparation_error?: false,
            error: if(socket.assigns.error == failure.error, do: nil, else: socket.assigns.error)
          )
        end
    end
  end

  defp clear_stale_retry(socket, snapshot) do
    socket =
      socket
      |> assign(
        preparing_video_id: nil,
        preparing_track: nil,
        pending_play: nil,
        failed_preparation: nil,
        preparation_error?: false,
        error: "Playback changed. Select this video again."
      )
      |> refresh_play_controls()

    if is_map(snapshot), do: apply_snapshot(socket, snapshot, false), else: socket
  end

  defp prepare_deferred_current(socket, completed_playback_id, failed_video_id \\ nil) do
    current = socket.assigns.current
    playback_id = socket.assigns.playback_id

    if is_map(current) and socket.assigns.playback_owner == socket.assigns.owner and
         playback_id != completed_playback_id and
         current["id"] != failed_video_id and
         socket.assigns.loaded_playback_id != playback_id and
         not preparing_media?(socket.assigns) and
         not failed_current_preparation?(socket.assigns, current, playback_id) do
      start_current_media(socket, current, playback_id)
    else
      socket
    end
  end

  defp result_dom_id(track), do: "result-#{track["id"]}"
  defp queue_dom_id(item), do: item.stream_id

  defp handle_backend_result(socket, {:search_result, generation}, {:ok, result}) do
    if generation == socket.assigns.search_generation,
      do: apply_search_result(socket, result),
      else: socket
  end

  defp handle_backend_result(socket, {:search_result, generation}, {:error, message}) do
    if generation == socket.assigns.search_generation,
      do: socket |> assign(searching?: false) |> assign_error(message),
      else: socket
  end

  defp handle_backend_result(socket, tag, {:ok, snapshot}) when is_map(snapshot) do
    socket = apply_snapshot(socket, snapshot, false)

    case tag do
      :queue_add_result -> assign(socket, error: nil, preparation_error?: false)
      :queue_result -> assign(socket, error: nil, preparation_error?: false)
      :playback_result -> assign(socket, error: nil, preparation_error?: false)
      :settings_result -> assign(socket, error: nil, preparation_error?: false)
      _ -> socket
    end
  end

  defp handle_backend_result(socket, _tag, {:error, message}), do: assign_error(socket, message)

  defp cancel_backend_search(socket) do
    owner = socket.assigns.owner
    generation = socket.assigns.search_generation + 1
    Task.start(fn -> Backend.request("cancel_search", %{"session" => owner}) end)

    assign(socket,
      search_generation: generation,
      searching?: false,
      error: nil
    )
  end

  defp setting_value(field, value) do
    case {field, value} do
      {field, "true"} when field in ["include_shorts", "sort_by_date", "auto_play_queue"] ->
        {:ok, true}

      {field, "false"} when field in ["include_shorts", "sort_by_date", "auto_play_queue"] ->
        {:ok, false}

      _ ->
        :error
    end
  end

  defp empty_snapshot do
    %{
      "queue" => [],
      "current" => nil,
      "playback_id" => 0,
      "owner" => nil,
      "revision" => 0,
      "settings" => @default_settings
    }
  end

  defp reset_worker_state(socket) do
    socket
    |> replace_results([])
    |> replace_queue([])
    |> assign(
      backend_epoch: socket.assigns.backend_epoch + 1,
      search_generation: socket.assigns.search_generation + 1,
      play_generation: socket.assigns.play_generation + 1,
      snapshot: empty_snapshot(),
      result_count: 0,
      queue_count: 0,
      current: nil,
      playback_id: 0,
      playback_owner: nil,
      settings: @default_settings,
      settings_form:
        to_form(%{"results_per_page" => @default_settings["results_per_page"]}, as: :settings),
      searching?: false,
      preparing_video_id: nil,
      preparing_track: nil,
      preparing_current: nil,
      preparing_current_track: nil,
      loaded_playback_id: nil,
      pending_play: nil,
      failed_preparation: nil,
      preparation_error?: false,
      backend_ready?: false
    )
    |> refresh_play_controls()
  end
end
