defmodule Youtui.Backend do
  @moduledoc "JSON packet bridge to the Rust `--backend-stdio` worker."
  use GenServer
  require Logger

  @topic "backend"
  @max_frame_bytes 1_048_576
  @default_restart_delays [250, 500, 1_000, 2_000, 4_000]
  @default_ready_timeout 15_000
  @stable_uptime 60_000

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  def request(command, params \\ %{}, timeout \\ 30_000)
      when is_binary(command) and is_map(params) and is_integer(timeout) and timeout > 0 do
    GenServer.call(__MODULE__, {:request, command, params, timeout}, timeout + 1_000)
  catch
    :exit, {:timeout, _} -> {:error, "backend request timed out"}
    :exit, reason -> {:error, "backend unavailable: #{inspect(reason)}"}
  end

  def subscribe, do: Phoenix.PubSub.subscribe(Youtui.PubSub, @topic)
  def ready?, do: GenServer.call(__MODULE__, :ready?)
  def cache_dir, do: GenServer.call(__MODULE__, :cache_dir)

  @impl true
  def init(opts) do
    Process.flag(:trap_exit, true)

    state = %{
      port: nil,
      ready?: false,
      cache_dir: nil,
      next_id: 1,
      pending: %{},
      restart_attempts: 0,
      restart_delays: Keyword.get(opts, :restart_delays, @default_restart_delays),
      ready_timeout: Keyword.get(opts, :ready_timeout, @default_ready_timeout),
      ready_timer: nil,
      stable_timer: nil
    }

    {:ok, open_worker(state)}
  end

  @impl true
  def handle_call(:ready?, _from, state), do: {:reply, state.ready?, state}
  def handle_call(:cache_dir, _from, state), do: {:reply, state.cache_dir, state}

  def handle_call({:request, _command, _params, _timeout}, _from, %{ready?: false} = state) do
    {:reply, {:error, "backend is not ready"}, state}
  end

  def handle_call({:request, command, params, timeout}, from, state) do
    id = state.next_id
    request = %{"version" => 1, "id" => id, "command" => command, "params" => params}

    with {:ok, payload} <- Jason.encode(request),
         true <- byte_size(payload) <= @max_frame_bytes,
         true <- Port.command(state.port, payload) do
      timer = Process.send_after(self(), {:request_timeout, id}, timeout)
      pending = Map.put(state.pending, id, %{from: from, timer: timer})
      {:noreply, %{state | pending: pending, next_id: id + 1}}
    else
      false ->
        {:reply, {:error, "backend frame exceeds 1 MiB or port is closed"}, state}

      {:error, reason} ->
        {:reply, {:error, "cannot encode backend request: #{inspect(reason)}"}, state}
    end
  end

  @impl true
  def handle_info({port, {:data, data}}, %{port: port} = state)
      when byte_size(data) > @max_frame_bytes do
    Logger.error("Rust backend sent a frame larger than 1 MiB")
    {:noreply, stop_worker_and_retry(state, port)}
  end

  def handle_info({port, {:data, data}}, %{port: port} = state) do
    case Jason.decode(data) do
      {:ok, %{"version" => 1, "event" => "ready"}} when state.ready? ->
        {:noreply, state}

      {:ok, %{"version" => 1, "event" => "ready"} = event} ->
        cache_dir = Map.get(event, "cache_dir") || get_in(event, ["result", "cache_dir"])

        case Youtui.Media.set_cache_dir(cache_dir) do
          {:ok, trusted_cache_dir} ->
            state = cancel_ready_timer(state)
            state = cancel_stable_timer(state)
            stable_ref = make_ref()

            stable_timer =
              Process.send_after(self(), {:worker_stable, stable_ref}, @stable_uptime)

            Phoenix.PubSub.broadcast(Youtui.PubSub, @topic, {:backend_event, event})

            {:noreply,
             %{
               state
               | ready?: true,
                 cache_dir: trusted_cache_dir,
                 stable_timer: {stable_ref, stable_timer}
             }}

          {:error, _reason} ->
            Logger.error("Rust backend announced an invalid media cache directory")
            {:noreply, stop_worker_and_retry(state, state.port)}
        end

      {:ok, %{"version" => 1, "event" => event_name} = event} when is_binary(event_name) ->
        Phoenix.PubSub.broadcast(Youtui.PubSub, @topic, {:backend_event, event})
        {:noreply, state}

      {:ok, %{"version" => 1, "id" => id, "ok" => ok} = response}
      when is_integer(id) and is_boolean(ok) ->
        case Map.pop(state.pending, id) do
          {nil, _pending} ->
            {:noreply, state}

          {%{from: from, timer: timer}, pending} ->
            Process.cancel_timer(timer)

            reply =
              if ok,
                do: {:ok, Map.get(response, "result", %{})},
                else: {:error, Map.get(response, "error", "backend request failed")}

            GenServer.reply(from, reply)
            {:noreply, %{state | pending: pending}}
        end

      other ->
        Logger.warning("ignoring invalid Rust backend frame: #{inspect(other)}")
        {:noreply, state}
    end
  end

  def handle_info({:request_timeout, id}, state) do
    case Map.pop(state.pending, id) do
      {nil, _pending} ->
        {:noreply, state}

      {%{from: from}, pending} ->
        GenServer.reply(from, {:error, "backend request timed out"})
        {:noreply, %{state | pending: pending}}
    end
  end

  def handle_info({port, {:exit_status, status}}, %{port: port} = state) do
    Logger.warning("Rust backend exited with status #{status}; restarting")
    {:noreply, state |> disconnect_worker("backend process exited") |> schedule_restart()}
  end

  def handle_info({:worker_ready_timeout, ref}, %{ready_timer: {ref, _timer}, port: port} = state) do
    Logger.error("Rust backend did not announce readiness before the startup deadline")
    {:noreply, stop_worker_and_retry(state, port)}
  end

  def handle_info({:worker_stable, ref}, %{stable_timer: {ref, _timer}} = state) do
    {:noreply, %{state | restart_attempts: 0, stable_timer: nil}}
  end

  def handle_info(:restart_worker, state), do: {:noreply, open_worker(state)}
  def handle_info(_message, state), do: {:noreply, state}

  @impl true
  def terminate(_reason, %{port: port} = state) when is_port(port) do
    fail_pending(state, "backend shutting down")
    Port.close(port)
    :ok
  end

  def terminate(_reason, state) do
    fail_pending(state, "backend shutting down")
    :ok
  end

  defp open_worker(state) do
    case System.get_env("YOUTUI_BIN") do
      nil ->
        Logger.warning("YOUTUI_BIN is unset; Rust backend is disabled")
        state

      executable when executable != "" ->
        if Path.type(executable) == :absolute and File.regular?(executable) do
          port =
            Port.open({:spawn_executable, executable}, [
              :binary,
              {:packet, 4},
              :exit_status,
              :use_stdio,
              :hide,
              args: ["--backend-stdio"]
            ])

          ref = make_ref()
          timer = Process.send_after(self(), {:worker_ready_timeout, ref}, state.ready_timeout)
          %{state | port: port, ready?: false, cache_dir: nil, ready_timer: {ref, timer}}
        else
          Logger.error("YOUTUI_BIN does not name a regular executable file")
          state
        end

      _ ->
        Logger.error("YOUTUI_BIN is empty; Rust backend is disabled")
        state
    end
  rescue
    error ->
      Logger.error("cannot start Rust backend: #{Exception.message(error)}")
      schedule_restart(state)
  end

  defp schedule_restart(%{restart_attempts: attempts, restart_delays: delays} = state) do
    case Enum.fetch(delays, attempts) do
      {:ok, delay} ->
        Process.send_after(self(), :restart_worker, delay)
        %{state | restart_attempts: attempts + 1}

      :error ->
        Logger.error("Rust backend restart limit reached; stopping retries")
        state
    end
  end

  defp disconnect_worker(state, reason) do
    state = state |> cancel_ready_timer() |> cancel_stable_timer()
    state = fail_pending(state, reason)
    :ok = Youtui.Media.set_cache_dir(nil)

    Phoenix.PubSub.broadcast(
      Youtui.PubSub,
      @topic,
      {:backend_event, %{"version" => 1, "event" => "disconnected"}}
    )

    %{state | port: nil, ready?: false, cache_dir: nil}
  end

  defp stop_worker_and_retry(state, port) do
    if is_port(port) and Port.info(port) != nil, do: Port.close(port)
    state |> disconnect_worker("backend worker failed readiness validation") |> schedule_restart()
  end

  defp cancel_ready_timer(%{ready_timer: nil} = state), do: state

  defp cancel_ready_timer(%{ready_timer: {_ref, timer}} = state) do
    Process.cancel_timer(timer)
    %{state | ready_timer: nil}
  end

  defp cancel_stable_timer(%{stable_timer: nil} = state), do: state

  defp cancel_stable_timer(%{stable_timer: {_ref, timer}} = state) do
    Process.cancel_timer(timer)
    %{state | stable_timer: nil}
  end

  defp fail_pending(state, message) do
    Enum.each(state.pending, fn {_id, %{from: from, timer: timer}} ->
      Process.cancel_timer(timer)
      GenServer.reply(from, {:error, message})
    end)

    %{state | pending: %{}}
  end
end
