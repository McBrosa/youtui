defmodule Youtui.Media do
  @moduledoc "Registers worker-created media files and exposes session-bound URLs."
  use GenServer

  @max_entries 64
  @id_pattern ~r/\A[A-Za-z0-9_-]{8,128}\z/

  def start_link(opts \\ []), do: GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  def set_cache_dir(path), do: GenServer.call(__MODULE__, {:set_cache_dir, path})
  def register(result), do: GenServer.call(__MODULE__, {:register, result})
  def lookup(media_id), do: GenServer.call(__MODULE__, {:lookup, media_id})

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
    path = Map.get(result, "path") || Map.get(result, :path)
    mime = Map.get(result, "mime") || Map.get(result, :mime)

    with true <- is_binary(media_id) and Regex.match?(@id_pattern, media_id),
         true <- mime == "video/mp4",
         {:ok, safe_path} <- safe_file(state.cache_dir, path) do
      entries = Map.put(state.entries, media_id, %{path: safe_path, mime: mime})
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
      {:ok, entry} ->
        case safe_file(state.cache_dir, entry.path) do
          {:ok, path} when path == entry.path ->
            {:reply, {:ok, entry}, state}

          _ ->
            entries = Map.delete(state.entries, media_id)
            order = Enum.reject(state.order, &(&1 == media_id))
            {:reply, :error, %{state | entries: entries, order: order}}
        end

      :error ->
        {:reply, :error, state}
    end
  end

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
