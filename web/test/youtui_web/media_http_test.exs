defmodule Youtui.MediaHTTPTest do
  use ExUnit.Case, async: false

  alias Plug.Conn
  alias Youtui.MediaHTTP

  @cdn "https://r1.example.googlevideo.com"

  setup do
    previous = Application.fetch_env(:youtui_web, :media_http_options)
    Application.put_env(:youtui_web, :media_http_options, plug: {Req.Test, __MODULE__})

    on_exit(fn ->
      case previous do
        {:ok, value} -> Application.put_env(:youtui_web, :media_http_options, value)
        :error -> Application.delete_env(:youtui_web, :media_http_options)
      end
    end)
  end

  test "follows validated relative redirects and preserves byte ranges" do
    previous = Req.default_options()
    Req.default_options(auth: {:bearer, "other-service"}, params: [secret: "private"])
    on_exit(fn -> Req.default_options(previous) end)

    Req.Test.stub(__MODULE__, fn conn ->
      assert conn.query_string == ""
      assert Conn.get_req_header(conn, "range") == ["bytes=0-2"]
      assert Conn.get_req_header(conn, "cookie") == []
      assert Conn.get_req_header(conn, "authorization") == []

      case conn.request_path do
        "/list/old" ->
          conn |> Conn.put_resp_header("location", "../video/new") |> Conn.send_resp(302, "")

        "/video/new" ->
          conn
          |> Conn.put_resp_header("content-type", "video/mp2t")
          |> Conn.put_resp_header("content-range", "bytes 0-2/25")
          |> Conn.send_resp(206, <<0, 1, 2>>)
      end
    end)

    assert {:ok,
            %{
              body: <<0, 1, 2>>,
              status: 206,
              url: @cdn <> "/video/new",
              content_type: "video/mp2t",
              content_range: "bytes 0-2/25"
            }} = MediaHTTP.fetch(@cdn <> "/list/old", 3, "bytes=0-2")
  end

  test "rejects invalid initial URL, range and redirect before sending a request there" do
    parent = self()

    Req.Test.stub(__MODULE__, fn conn ->
      send(parent, {:fetched, conn.host})
      conn |> Conn.put_resp_header("location", "https://example.org/") |> Conn.send_resp(307, "")
    end)

    assert {:error, _} = MediaHTTP.fetch("https://example.org/", 99)
    assert {:error, :invalid_range} = MediaHTTP.fetch(@cdn <> "/video", 99, "bytes=3-4,8-9")
    refute_received {:fetched, _}
    assert {:error, _} = MediaHTTP.fetch(@cdn <> "/video", 99)
    assert_received {:fetched, "r1.example.googlevideo.com"}
    refute_received {:fetched, _}
  end

  test "rejects a chunked response as soon as cumulative bytes exceed the limit" do
    Req.Test.stub(__MODULE__, fn conn ->
      conn = Conn.send_chunked(conn, 200)
      {:ok, conn} = Conn.chunk(conn, "abc")
      {:ok, conn} = Conn.chunk(conn, "def")
      conn
    end)

    assert {:error, :body_too_large} = MediaHTTP.fetch(@cdn <> "/video", 5)
  end

  test "rejects a redirect with an explicit port before URL normalization" do
    parent = self()

    Req.Test.stub(__MODULE__, fn conn ->
      send(parent, :requested)

      conn
      |> Conn.put_resp_header("location", @cdn <> ":443/next")
      |> Conn.send_resp(302, "")
    end)

    assert {:error, _} = MediaHTTP.fetch(@cdn <> "/start", 64)
    assert_received :requested
    refute_received :requested
  end

  test "preserves an unsatisfiable range response" do
    Req.Test.stub(__MODULE__, fn conn -> Conn.send_resp(conn, 416, "") end)
    assert {:error, :invalid_range} = MediaHTTP.fetch(@cdn <> "/video", 64, "bytes=100-")
  end

  test "stops following a redirect loop" do
    parent = self()

    Req.Test.stub(__MODULE__, fn conn ->
      send(parent, :fetched)
      conn |> Conn.put_resp_header("location", "/again") |> Conn.send_resp(302, "")
    end)

    assert {:error, _} = MediaHTTP.fetch(@cdn <> "/start", 64)
    for _ <- 1..4, do: assert_received(:fetched)
    refute_received :fetched
  end
end
