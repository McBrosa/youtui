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

  test "authenticated media requests redirect to the worker's progressive stream", %{conn: conn} do
    url = "https://rr2.googlevideo.com/videoplayback?sig=x%2Fy"
    media = %{"media_id" => "stream001", "mime" => "video/mp4", "stream_url" => url}
    assert {:ok, path} = Youtui.Media.register(media)
    assert conn |> get(path) |> response(401) == "authentication required"

    conn = conn |> init_test_session(%{youtui_session: "signed-session"}) |> get(path)
    assert response(conn, 307) == ""
    assert get_resp_header(conn, "location") == [url]
    assert get_resp_header(conn, "cache-control") == ["private, no-store"]
    assert get_resp_header(conn, "referrer-policy") == ["no-referrer"]
  end

  test "media registration limits progressive destinations to HTTPS CDN hosts" do
    for url <- [
          "http://rr2.googlevideo.com/videoplayback",
          "https://googlevideo.com.example.org/videoplayback",
          "https://outside.example/",
          "https://user@rr2.googlevideo.com/videoplayback",
          "https://rr2.googlevideo.com:443/videoplayback",
          "https://rr2.googlevideo.com/videoplayback#fragment"
        ] do
      assert {:error, _} =
               Youtui.Media.register(%{
                 "media_id" => "stream001",
                 "mime" => "video/mp4",
                 "stream_url" => url
               })
    end
  end

  test "authenticated HLS master pairs the selected video and audio playlists", %{conn: conn} do
    video = "https://manifest.googlevideo.com/video.m3u8?sig=foo%2Fbar"
    audio = "https://manifest.googlevideo.com/audio.m3u8?sig=foo&part=1"

    media = %{
      "media_id" => "hls00001",
      "mime" => "application/vnd.apple.mpegurl",
      "video_playlist_url" => video,
      "audio_playlist_url" => audio,
      "bandwidth" => 2_800_000,
      "width" => 1280,
      "height" => 720,
      "video_codec" => "avc1.64001F",
      "audio_codec" => "mp4a.40.2"
    }

    assert {:ok, path} = Youtui.Media.register(media)
    assert conn |> get(path) |> response(401) == "authentication required"
    conn = conn |> init_test_session(%{youtui_session: "signed-session"}) |> get(path)
    assert get_resp_header(conn, "cache-control") == ["private, no-store"]
    assert get_resp_header(conn, "referrer-policy") == ["no-referrer"]
    assert get_resp_header(conn, "content-type") == ["application/vnd.apple.mpegurl"]

    assert response(conn, 200) == """
           #EXTM3U
           #EXT-X-VERSION:4
           #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="Audio",DEFAULT=YES,AUTOSELECT=YES,URI="#{path}/audio/index.m3u8"
           #EXT-X-STREAM-INF:BANDWIDTH=2800000,AUDIO="audio",CODECS="avc1.64001F,mp4a.40.2",RESOLUTION=1280x720
           #{path}/video/index.m3u8
           """

    for invalid <- [
          %{"bandwidth" => -1},
          %{"bandwidth" => "2800000"},
          %{"audio_playlist_url" => audio <> "\""},
          %{"video_playlist_url" => video <> "\n#EXTM3U"},
          %{"audio_playlist_url" => "https://untrusted.example/audio"}
        ] do
      assert {:error, _} = Youtui.Media.register(Map.merge(media, invalid))
    end

    assert {:ok, ^path} = Youtui.Media.register(Map.drop(media, ["height", "audio_codec"]))
    assert {:ok, %{playlist: playlist}} = Youtui.Media.lookup("hls00001")
    refute playlist =~ "CODECS"
    refute playlist =~ "RESOLUTION"
  end

  describe "authenticated HLS renditions" do
    setup %{conn: conn} do
      {:ok, path} =
        Youtui.Media.register(%{
          "media_id" => "hls00002",
          "mime" => "application/vnd.apple.mpegurl",
          "video_playlist_url" => "https://manifest.googlevideo.com/v/index.m3u8",
          "audio_playlist_url" => "https://manifest.googlevideo.com/a/index.m3u8",
          "bandwidth" => 1_500_000
        })

      old = Application.fetch_env(:youtui_web, :media_http_options)

      on_exit(fn ->
        case old do
          {:ok, options} -> Application.put_env(:youtui_web, :media_http_options, options)
          :error -> Application.delete_env(:youtui_web, :media_http_options)
        end
      end)

      %{path: path, authenticated: init_test_session(conn, %{youtui_session: "signed-session"})}
    end

    test "rewrites playlists and map/key references, forwards range and caches each rendition", %{
      conn: anon,
      authenticated: auth,
      path: path
    } do
      parent = self()

      playlist = """
      #EXTM3U
      #EXT-X-VERSION:6
      #EXT-X-MAP:URI="init.mp4",BYTERANGE="2@0"
      #EXT-X-KEY:METHOD=AES-128,URI="/key"
      #EXTINF:4,
      https://rr2.googlevideo.com/seg?sig=test%2Fkey
      #EXT-X-ENDLIST
      """

      stub_media(fn conn ->
        send(
          parent,
          {:fetched, conn.host, conn.request_path, conn.query_string,
           get_req_header(conn, "range")}
        )

        case conn.request_path do
          "/v/index.m3u8" ->
            send_resp(conn, 200, playlist)

          "/a/index.m3u8" ->
            send_resp(conn, 200, "#EXTM3U\n#EXTINF:4,\naudio.mp4\n#EXT-X-ENDLIST\n")

          "/v/init.mp4" ->
            send_resp(conn, 200, "init")

          "/key" ->
            send_resp(conn, 200, "key")

          "/seg" ->
            conn
            |> put_resp_header("content-range", "bytes 2-4/8")
            |> put_resp_content_type("video/mp4", nil)
            |> send_resp(206, "234")

          "/a/audio.mp4" ->
            send_resp(conn, 200, "audio")
        end
      end)

      for suffix <- ["/video/index.m3u8", "/video/0"] do
        assert anon |> get(path <> suffix) |> response(401) == "authentication required"
      end

      assert auth |> get(path <> "/video/0") |> response(404) == "media not found"
      refute_received {:fetched, _, _, _, _}

      local = auth |> get(path <> "/video/index.m3u8") |> response(200)
      assert local =~ ~s(#EXT-X-MAP:URI="0",BYTERANGE="2@0")
      assert local =~ ~s(#EXT-X-KEY:METHOD=AES-128,URI="1")
      assert local =~ "\n2\n"
      refute local =~ "googlevideo"
      assert_received {:fetched, "manifest.googlevideo.com", "/v/index.m3u8", "", []}
      assert auth |> get(path <> "/video/index.m3u8") |> response(200) == local
      refute_received {:fetched, _, _, _, _}
      assert auth |> get(path <> "/video/0") |> response(200) == "init"
      assert auth |> get(path <> "/video/1") |> response(200) == "key"
      assert_received {:fetched, _, "/v/init.mp4", "", []}
      assert_received {:fetched, _, "/key", "", []}
      seg = auth |> put_req_header("range", "bytes=2-4") |> get(path <> "/video/2")
      assert response(seg, 206) == "234"
      assert get_resp_header(seg, "content-range") == ["bytes 2-4/8"]
      assert get_resp_header(seg, "content-type") == ["video/mp4"]
      assert_received {:fetched, "rr2.googlevideo.com", "/seg", "sig=test%2Fkey", ["bytes=2-4"]}
      assert auth |> get(path <> "/audio/index.m3u8") |> response(200) =~ "\n0\n"
      assert auth |> get(path <> "/audio/0") |> response(200) == "audio"

      for suffix <- ["/video/3", "/video/000", "/video/-1", "/unused/index.m3u8"] do
        assert auth |> get(path <> suffix) |> response(404) == "media not found"
      end

      assert auth
             |> put_req_header("range", "bytes=1-2,3-4")
             |> get(path <> "/video/0")
             |> response(416) == ""
    end

    test "rejects malformed, untrusted and oversized playlists before registering assets", %{
      authenticated: auth,
      path: path
    } do
      for element <- [
            "https://outside.example/seg",
            "//user@rr2.googlevideo.com/seg",
            "https://rr2.googlevideo.com:443/seg",
            ~s(#EXT-X-MAP:URI="https://outside.example/init"),
            ~s(#EXT-X-KEY:METHOD=AES-128,URI="one",URI="two"),
            "#EXT-X-STREAM-INF:BANDWIDTH=1",
            "#EXTINF:4,\rhttps://outside.example/seg",
            String.duplicate("seg\n", 4097)
          ] do
        body = "#EXTM3U\n#EXTINF:4,\n" <> element <> "\n#EXT-X-ENDLIST\n"
        stub_media(fn conn -> send_resp(conn, 200, body) end)

        assert auth |> get(path <> "/video/index.m3u8") |> response(502) ==
                 "media playlist unavailable"

        assert auth |> get(path <> "/video/0") |> response(404) == "media not found"
      end
    end

    test "handles expired segments, excessive responses and malformed partial responses", %{
      authenticated: auth,
      path: path
    } do
      stub_media(fn conn ->
        send_resp(conn, 200, "#EXTM3U\n#EXTINF:4,\nseg\n#EXT-X-ENDLIST\n")
      end)

      assert auth |> get(path <> "/video/index.m3u8") |> response(200) =~ "\n0\n"

      for {status, body} <- [
            {403, "expired"},
            {206, "partial without byte positions"},
            {200, :binary.copy("x", 16 * 1024 * 1024 + 1)}
          ] do
        stub_media(fn conn -> send_resp(conn, status, body) end)
        assert auth |> get(path <> "/video/0") |> response(502) == "media segment unavailable"
      end

      stub_media(fn conn -> send_resp(conn, 416, "") end)

      assert auth
             |> put_req_header("range", "bytes=9000-")
             |> get(path <> "/video/0")
             |> response(416) == ""
    end
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
  defp stub_media(plug), do: Application.put_env(:youtui_web, :media_http_options, plug: plug)
end
