defmodule YoutuiWeb.BackendTest do
  use ExUnit.Case, async: false

  test "packet4 worker readiness and correlated request response" do
    root = Path.expand("../../tmp/fake-backend-cache", __DIR__)
    File.mkdir_p!(root)
    previous_bin = System.get_env("YOUTUI_BIN")
    previous_cache = System.get_env("YOUTUI_FAKE_CACHE")

    on_exit(fn ->
      Supervisor.terminate_child(Youtui.Supervisor, Youtui.Backend)
      Supervisor.delete_child(Youtui.Supervisor, Youtui.Backend)
      restore_env("YOUTUI_BIN", previous_bin)
      restore_env("YOUTUI_FAKE_CACHE", previous_cache)
      Supervisor.start_child(Youtui.Supervisor, Youtui.Backend)
      File.rm_rf!(root)
    end)

    :ok = Youtui.Backend.subscribe()
    :ok = Supervisor.terminate_child(Youtui.Supervisor, Youtui.Backend)
    :ok = Supervisor.delete_child(Youtui.Supervisor, Youtui.Backend)

    System.put_env("YOUTUI_BIN", Path.expand("../support/fake_backend.js", __DIR__))
    System.put_env("YOUTUI_FAKE_CACHE", root)
    {:ok, _pid} = Supervisor.start_child(Youtui.Supervisor, Youtui.Backend)

    assert_receive {:backend_event, %{"event" => "ready", "cache_dir" => ^root}}, 2_000
    assert Youtui.Backend.ready?()
    assert {:ok, %{"version" => 1}} = Youtui.Backend.request("ping")
  end

  test "invalid worker cache roots never report ready" do
    root = Path.expand("tmp/fake-backend-cache-root")
    alias_path = Path.expand("tmp/fake-backend-cache-alias")
    File.mkdir_p!(root)
    File.ln_s!(root, alias_path)

    configure_worker(
      Path.expand("../support/fake_backend.js", __DIR__),
      alias_path,
      [],
      [root, alias_path]
    )

    assert_receive {:backend_event, %{"event" => "disconnected"}}, 2_000
    refute Youtui.Backend.ready?()
    state = :sys.get_state(Youtui.Backend)
    assert state.cache_dir == nil
    assert state.port == nil
  end

  test "crashes immediately after ready exhaust the bounded restart budget" do
    root = Path.expand("tmp/fake-backend-cache")
    File.mkdir_p!(root)

    configure_worker(
      Path.expand("../support/crash_after_ready.js", __DIR__),
      root,
      [restart_delays: List.duplicate(1, 5)],
      [root]
    )

    for _attempt <- 1..6 do
      assert_receive {:backend_event, %{"event" => "ready"}}, 2_000
      assert_receive {:backend_event, %{"event" => "disconnected"}}, 2_000
    end

    state = :sys.get_state(Youtui.Backend)
    assert state.restart_attempts == 5
    assert state.port == nil
    refute Youtui.Backend.ready?()
  end

  defp configure_worker(executable, cache_dir, opts, cleanup_paths) do
    previous_bin = System.get_env("YOUTUI_BIN")
    previous_cache = System.get_env("YOUTUI_FAKE_CACHE")

    on_exit(fn ->
      Supervisor.terminate_child(Youtui.Supervisor, Youtui.Backend)
      Supervisor.delete_child(Youtui.Supervisor, Youtui.Backend)
      restore_env("YOUTUI_BIN", previous_bin)
      restore_env("YOUTUI_FAKE_CACHE", previous_cache)
      Supervisor.start_child(Youtui.Supervisor, Youtui.Backend)
      Enum.each(cleanup_paths, &File.rm_rf!/1)
    end)

    :ok = Youtui.Backend.subscribe()
    :ok = Supervisor.terminate_child(Youtui.Supervisor, Youtui.Backend)
    :ok = Supervisor.delete_child(Youtui.Supervisor, Youtui.Backend)
    System.put_env("YOUTUI_BIN", executable)
    System.put_env("YOUTUI_FAKE_CACHE", cache_dir)
    {:ok, _pid} = Supervisor.start_child(Youtui.Supervisor, {Youtui.Backend, opts})
  end

  defp restore_env(name, nil), do: System.delete_env(name)
  defp restore_env(name, value), do: System.put_env(name, value)
end
