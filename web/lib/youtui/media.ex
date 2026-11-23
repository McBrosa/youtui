defmodule Youtui.Media do
  @moduledoc "Registers worker-prepared media for authenticated browser playback."
  use GenServer

  @max_entries 64
  @id_pattern ~r/\A[A-Za-z0-9_-]{8,128}\z/

  def start_link(opts \\ []), do: GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  def set_cache_dir(path), do: GenServer.call(__MODULE__, {:set_cache_dir, path})
  def register(result), do: GenServer.call(__MODULE__, {:register, result})
  def lookup(media_id), do: GenServer.call(__MODULE__, {:lookup, media_id})
  def rendition(id, kind), do: GenServer.call(__MODULE__, {:rendition, id, kind})

  def store_rendition(id, kind, url, rendition),
    do: GenServer.call(__MODULE__, {:store_rendition, id, kind, url, rendition})

  def asset(id, kind, index), do: GenServer.call(__MODULE__, {:asset, id, kind, index})

  @impl true
  def init(_opts), do: {:ok, %{cache_dir: nil, entries: %{}, order: []}}

  @impl true
  def handle_call({:set_cache_dir, path}, _from, state) do
    state = %{state | cache_dir: nil, entries: %{}, order: []}

    case trusted_root(path) do
      {:ok, root} ->
        {:reply, {:ok, root}, %{state | cache_dir: root}}

      :error when is_nil(path) ->
        {:reply, :ok, state}

      :error ->
        {:reply, {:error, "worker cache directory is not a canonical safe directory"}, state}
    end
  end

  def handle_call({:register, result}, _from, state) when is_map(result) do
    media_id = Map.get(result, "media_id") || Map.get(result, :media_id)
    mime = Map.get(result, "mime") || Map.get(result, :mime)

    with true <- is_binary(media_id) and Regex.match?(@id_pattern, media_id),
         {:ok, entry} <- media_entry(state.cache_dir, mime, result, media_id) do
      entries = Map.put(state.entries, media_id, Map.put(entry, :mime, mime))
      order = [media_id | Enum.reject(state.order, &(&1 == media_id))]
      {entries, order} = trim(entries, order)
      {:reply, {:ok, "/media/#{media_id}"}, %{state | entries: entries, order: order}}
    else
      _ -> {:reply, {:error, "media result is invalid or outside the worker cache"}, state}
    end
  end

  def handle_call({:register, _}, _from, state),
    do: {:reply, {:error, "media result must be a map"}, state}

  def handle_call({:lookup, media_id}, _from, state) do
    case Map.fetch(state.entries, media_id) do
      {:ok, %{path: _} = entry} ->
        case safe_file(state.cache_dir, entry.path) do
          {:ok, path} when path == entry.path ->
            {:reply, {:ok, entry}, state}

          _ ->
            entries = Map.delete(state.entries, media_id)
            order = Enum.reject(state.order, &(&1 == media_id))
            {:reply, :error, %{state | entries: entries, order: order}}
        end

      {:ok, %{playlist: playlist, mime: mime}} ->
        {:reply, {:ok, %{playlist: playlist, mime: mime}}, state}

      {:ok, entry} ->
        {:reply, {:ok, entry}, state}

      :error ->
        {:reply, :error, state}
    end
  end

  def handle_call({:rendition, id, kind}, _from, state) do
    result =
      case get_in(state.entries, [id, :sources, kind]) do
        %{rendition: %{playlist: playlist}} -> {:ok, playlist}
        %{url: url} -> {:fetch, url}
        _ -> :error
      end

    {:reply, result, state}
  end

  def handle_call({:store_rendition, id, kind, url, rendition}, _from, state) do
    case get_in(state.entries, [id, :sources, kind]) do
      %{url: ^url, rendition: %{playlist: playlist}} ->
        {:reply, {:ok, playlist}, state}

      %{url: ^url} ->
        {:reply, {:ok, rendition.playlist},
         put_in(state, [:entries, id, :sources, kind, :rendition], rendition)}

      _ ->
        {:reply, :error, state}
    end
  end

  def handle_call({:asset, id, kind, index}, _from, state) do
    case get_in(state.entries, [id, :sources, kind, :rendition, :assets]) do
      assets
      when is_tuple(assets) and is_integer(index) and index >= 0 and index < tuple_size(assets) ->
        {:reply, {:ok, elem(assets, index)}, state}

      _ ->
        {:reply, :error, state}
    end
  end

  defp media_entry(root, "video/mp4", result, _id) when is_binary(root) do
    path = result["path"] || result[:path]
    stream = result["stream_url"] || result[:stream_url]

    case {path, stream} do
      {nil, url} ->
        with :ok <- check_cdn_url(url), do: {:ok, %{stream_url: url}}

      {path, nil} ->
        with {:ok, path} <- safe_file(root, path), do: {:ok, %{path: path}}

      _ ->
        :error
    end
  end

  defp media_entry(
         root,
         "application/vnd.apple.mpegurl",
         %{
           "video_playlist_url" => video_url,
           "audio_playlist_url" => audio_url,
           "bandwidth" => bandwidth
         } = result,
         id
       )
       when is_binary(root) and is_integer(bandwidth) and bandwidth in 1..0xFFFFFFFFFFFFFFFF do
    with :ok <- check_cdn_url(video_url),
         :ok <- check_cdn_url(audio_url) do
      metadata = Youtui.HLS.variant_attributes(result)

      playlist = """
      #EXTM3U
      #EXT-X-VERSION:4
      #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="Audio",DEFAULT=YES,AUTOSELECT=YES,URI="/media/#{id}/audio/index.m3u8"
      #EXT-X-STREAM-INF:BANDWIDTH=#{bandwidth},AUDIO="audio"#{metadata}
      /media/#{id}/video/index.m3u8
      """

      {:ok,
       %{
         playlist: playlist,
         sources: %{"audio" => %{url: audio_url}, "video" => %{url: video_url}}
       }}
    end
  end

  defp media_entry(_, _, _, _), do: :error

  def check_cdn_url(url) when is_binary(url) and byte_size(url) <= 16_384 do
    with {:ok, %URI{scheme: "https", host: host, port: 443, userinfo: nil, fragment: nil}}
         when is_binary(host) <- URI.new(url),
         true <- Regex.match?(~r/\A(?:[a-z0-9-]+\.)+googlevideo\.com\z/i, host),
         true <- String.starts_with?(url, "https://#{host}/"),
         false <- Regex.match?(~r/["\\\x00-\x20\x7F]/, url) do
      :ok
    else
      _ -> :error
    end
  end

  def check_cdn_url(_), do: :error

  def resolve_cdn_url(_base, "https://" <> _ = url), do: checked_url(url)
  def resolve_cdn_url(_base, "//" <> _ = ref), do: checked_url("https:" <> ref)

  def resolve_cdn_url(base, ref) when is_binary(ref) and byte_size(ref) in 1..16_384 do
    with false <- Regex.match?(~r/["\\\x00-\x20\x7F]/, ref),
         {:ok, %URI{scheme: nil, host: nil, fragment: nil} = relative} <- URI.new(ref) do
      base |> URI.merge(relative) |> URI.to_string() |> checked_url()
    else
      _ -> :error
    end
  end

  def resolve_cdn_url(_, _), do: :error

  defp checked_url(url), do: with(:ok <- check_cdn_url(url), do: {:ok, url})

  defp trusted_root(path) when is_binary(path) do
    with true <- Path.type(path) == :absolute,
         true <- Path.expand(path) == path,
         :ok <- no_symlink_path_components(path),
         {:ok, stat} <- File.lstat(path),
         true <- stat.type == :directory do
      {:ok, path}
    else
      _ -> :error
    end
  end

  defp trusted_root(_), do: :error

  defp safe_file(root, path) when is_binary(root) and is_binary(path) do
    expanded = Path.expand(path)
    relative = Path.relative_to(expanded, root)

    with {:ok, ^root} <- trusted_root(root),
         true <- Path.type(path) == :absolute,
         true <-
           relative != expanded and relative not in ["", "."] and
             not String.starts_with?(relative, "../"),
         :ok <- no_symlink_components(root, relative),
         {:ok, stat} <- File.lstat(expanded),
         true <- stat.type == :regular,
         true <- Path.extname(expanded) == ".mp4" do
      {:ok, expanded}
    else
      _ -> {:error, :unsafe_file}
    end
  end

  defp safe_file(_, _), do: {:error, :no_cache_root}

  defp no_symlink_components(root, relative) do
    parts = Path.split(relative)

    Enum.reduce_while(parts, root, fn part, current ->
      next = Path.join(current, part)

      case File.lstat(next) do
        {:ok, %{type: type}} when type in [:directory, :regular] -> {:cont, next}
        _ -> {:halt, :error}
      end
    end)
    |> case do
      :error -> {:error, :unsafe_component}
      _ -> :ok
    end
  end

  defp no_symlink_path_components(path) do
    path
    |> String.split("/", trim: true)
    |> Enum.reduce_while("/", fn part, current ->
      next = Path.join(current, part)

      case File.lstat(next) do
        {:ok, %{type: :directory}} -> {:cont, next}
        _ -> {:halt, :error}
      end
    end)
    |> case do
      :error -> {:error, :unsafe_component}
      _ -> :ok
    end
  end

  defp trim(entries, order) when length(order) <= @max_entries, do: {entries, order}

  defp trim(entries, order) do
    {kept, removed} = Enum.split(order, @max_entries)
    {Map.drop(entries, removed), kept}
  end
end
