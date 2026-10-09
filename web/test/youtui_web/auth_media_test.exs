defmodule YoutuiWeb.AuthMediaTest do
  use YoutuiWeb.ConnCase, async: false

  setup %{conn: conn} do
    root = Path.expand("tmp/youtui-media-test-#{System.unique_integer([:positive])}")
    File.mkdir_p!(root)
    media_path = Path.join(root, "m0000000000000001.mp4")
    File.write!(media_path, "0123456789")
    assert {:ok, ^root} = Youtui.Media.set_cache_dir(root)

    {:ok, media_url} =
      Youtui.Media.register(%{
        "media_id" => "m0000000000000001",
        "path" => media_path,
        "mime" => "video/mp4"
      })

    on_exit(fn ->
      Youtui.Media.set_cache_dir(nil)
      File.rm_rf!(root)
    end)

    {:ok, conn: conn, media_url: media_url, root: root}
  end

  test "health requires the launch token and reports backend readiness", %{conn: conn} do
    previous = System.get_env("YOUTUI_WEB_TOKEN")
    System.put_env("YOUTUI_WEB_TOKEN", "test-launch-token")

    on_exit(fn -> restore_env("YOUTUI_WEB_TOKEN", previous) end)

    assert conn |> get("/health") |> response(401) == ~s({"ready":false})

    conn = conn |> put_req_header("x-youtui-token", "test-launch-token") |> get("/health")
    assert response(conn, 503) == ~s({"ready":false})
  end

  test "backend requests stay closed until the worker sends ready" do
    refute Youtui.Backend.ready?()
    assert {:error, "backend is not ready"} = Youtui.Backend.request("ping")
  end

  test "start exchanges the bootstrap token for a signed session", %{conn: conn} do
    previous = System.get_env("YOUTUI_WEB_TOKEN")
    System.put_env("YOUTUI_WEB_TOKEN", "test-launch-token")

    on_exit(fn -> restore_env("YOUTUI_WEB_TOKEN", previous) end)

    conn = get(conn, "/start?token=test-launch-token")
    assert redirected_to(conn) == "/"
    assert is_binary(get_session(conn, :youtui_session))
    assert get_resp_header(conn, "cache-control") == ["no-store"]
    assert get_resp_header(conn, "referrer-policy") == ["no-referrer"]
  end

  test "the launch token can create separate sessions for multiple tabs" do
    previous = System.get_env("YOUTUI_WEB_TOKEN")
    System.put_env("YOUTUI_WEB_TOKEN", "multi-tab-token")

    on_exit(fn -> restore_env("YOUTUI_WEB_TOKEN", previous) end)

    assert {:ok, first_session} = YoutuiWeb.Auth.exchange("multi-tab-token")
    assert {:ok, second_session} = YoutuiWeb.Auth.exchange("multi-tab-token")
    refute first_session == second_session
  end

  test "media requires a session and serves byte ranges", %{conn: conn, media_url: media_url} do
    assert conn |> get(media_url) |> response(401) == "authentication required"

    conn =
      conn
      |> init_test_session(%{youtui_session: "signed-session"})
      |> put_req_header("range", "bytes=2-5")
      |> get(media_url)

    assert response(conn, 206) == "2345"
    assert get_resp_header(conn, "content-range") == ["bytes 2-5/10"]
    assert get_resp_header(conn, "accept-ranges") == ["bytes"]
  end

  test "invalid ranges return 416", %{conn: conn, media_url: media_url} do
    conn =
      conn
      |> init_test_session(%{youtui_session: "signed-session"})
      |> put_req_header("range", "bytes=20-30")
      |> get(media_url)

    assert response(conn, 416) == ""
    assert get_resp_header(conn, "content-range") == ["bytes */10"]
  end

  test "ranges outside the byte grammar return 416", %{conn: conn, media_url: media_url} do
    conn =
      conn
      |> init_test_session(%{youtui_session: "signed-session"})
      |> put_req_header("range", "bytes=+2-5")
      |> get(media_url)

    assert response(conn, 416) == ""
  end

  test "HEAD exposes metadata without streaming the file", %{conn: conn, media_url: media_url} do
    conn =
      conn
      |> init_test_session(%{youtui_session: "signed-session"})
      |> head(media_url)

    assert response(conn, 200) == ""
    assert get_resp_header(conn, "content-length") == ["10"]
  end

  test "media registry rejects paths outside the trusted cache", _context do
    outside = Path.expand("tmp/outside-#{System.unique_integer([:positive])}.mp4")
    File.write!(outside, "not trusted")
    on_exit(fn -> File.rm(outside) end)

    assert {:error, _} =
             Youtui.Media.register(%{
               "media_id" => "m0000000000000002",
               "path" => outside,
               "mime" => "video/mp4"
             })
  end

  test "media lookup rejects a file replaced with a symlink", %{
    conn: conn,
    root: root,
    media_url: media_url
  } do
    path = Path.join(root, "m0000000000000001.mp4")
    outside = Path.expand("tmp/outside-link-target-#{System.unique_integer([:positive])}.mp4")
    File.write!(outside, "untrusted data")
    on_exit(fn -> File.rm(outside) end)
    File.rm!(path)
    File.ln_s!(outside, path)

    conn =
      conn
      |> init_test_session(%{youtui_session: "signed-session"})
      |> get(media_url)

    assert response(conn, 404) == "media not found"
  end

  test "media registry rejects a symlinked cache directory", %{root: root} do
    link = Path.expand("tmp/youtui-cache-link-#{System.unique_integer([:positive])}")
    File.ln_s!(root, link)
    on_exit(fn -> File.rm(link) end)

    assert {:error, _} = Youtui.Media.set_cache_dir(link)
    assert {:error, _} = Youtui.Media.register(%{})
  end

  test "media lookup rejects a cache root replaced with a symlink", %{
    conn: conn,
    root: root,
    media_url: media_url
  } do
    original = root <> "-original"
    outside = root <> "-outside"
    File.mkdir_p!(outside)
    File.write!(Path.join(outside, "m0000000000000001.mp4"), "untrusted data")
    File.rename!(root, original)
    File.ln_s!(outside, root)

    on_exit(fn ->
      File.rm(root)
      File.rm_rf!(original)
      File.rm_rf!(outside)
    end)

    conn =
      conn
      |> init_test_session(%{youtui_session: "signed-session"})
      |> get(media_url)

    assert response(conn, 404) == "media not found"
  end

  defp restore_env(name, nil), do: System.delete_env(name)
  defp restore_env(name, value), do: System.put_env(name, value)
end
