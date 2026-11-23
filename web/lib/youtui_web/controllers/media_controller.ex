defmodule YoutuiWeb.MediaController do
  use YoutuiWeb, :controller

  alias Youtui.{HLS, Media, MediaHTTP}

  @range_pattern ~r/\A[0-9]*-[0-9]*\z/
  @segment_bytes 16 * 1024 * 1024
  @asset_types ~w(video/mp2t video/mp4 audio/mp4 audio/aac audio/mpeg video/iso.segment audio/iso.segment)

  def show(conn, %{"media_id" => media_id}) do
    with {:ok, %{path: path, mime: mime}} <- Media.lookup(media_id),
         {:ok, %{size: size}} <- File.stat(path) do
      serve(conn, path, mime, size, get_req_header(conn, "range") |> List.first())
    else
      {:ok, %{stream_url: url}} ->
        conn
        |> put_resp_header("location", url)
        |> private_media_headers()
        |> send_resp(:temporary_redirect, "")

      {:ok, %{playlist: playlist}} ->
        send_playlist(conn, playlist)

      _ ->
        send_resp(conn, :not_found, "media not found")
    end
  end

  def rendition(conn, %{"media_id" => id, "kind" => kind}) do
    case HLS.rendition(id, kind) do
      {:ok, playlist} -> send_playlist(conn, playlist)
      :error -> send_resp(conn, :not_found, "media not found")
      {:error, _} -> send_resp(conn, :bad_gateway, "media playlist unavailable")
    end
  end

  def asset(conn, %{"media_id" => id, "kind" => kind, "index" => index}) do
    with true <- Regex.match?(~r/\A(?:0|[1-9]\d{0,3})\z/, index),
         {:ok, url} <- Media.asset(id, kind, String.to_integer(index)),
         {:ok, range} <- asset_range(get_req_header(conn, "range")),
         {:ok, response} <- MediaHTTP.fetch(url, @segment_bytes, range),
         {:ok, conn} <- asset_content_range(conn, response) do
      conn
      |> private_media_headers()
      |> put_resp_header("x-content-type-options", "nosniff")
      |> put_resp_header("content-security-policy", "sandbox; default-src 'none'")
      |> put_resp_header("accept-ranges", "bytes")
      |> put_resp_content_type(asset_type(response.content_type), nil)
      |> send_resp(response.status, response.body)
    else
      {:error, :invalid_range} -> send_resp(conn, :requested_range_not_satisfiable, "")
      {:error, _} -> send_resp(conn, :bad_gateway, "media segment unavailable")
      _ -> send_resp(conn, :not_found, "media not found")
    end
  end

  defp send_playlist(conn, playlist) do
    conn
    |> private_media_headers()
    |> put_resp_content_type("application/vnd.apple.mpegurl", nil)
    |> send_resp(:ok, playlist)
  end

  defp asset_range([]), do: {:ok, nil}
  defp asset_range([range]), do: {:ok, range}
  defp asset_range(_), do: {:error, :invalid_range}

  defp asset_content_range(conn, %{status: 200}), do: {:ok, conn}

  defp asset_content_range(conn, %{status: 206, content_range: range})
       when is_binary(range) and byte_size(range) <= 80 do
    if Regex.match?(~r/\Abytes \d+-\d+\/(?:\d+|\*)\z/, range),
      do: {:ok, put_resp_header(conn, "content-range", range)},
      else: {:error, :bad_upstream_range}
  end

  defp asset_content_range(_, _), do: {:error, :bad_upstream_range}

  defp asset_type(type) when type in @asset_types, do: type
  defp asset_type(_), do: "application/octet-stream"

  defp private_media_headers(conn) do
    conn
    |> put_resp_header("cache-control", "private, no-store")
    |> put_resp_header("referrer-policy", "no-referrer")
  end

  defp serve(conn, path, mime, size, nil) do
    conn
    |> put_resp_header("accept-ranges", "bytes")
    |> put_resp_header("content-length", Integer.to_string(size))
    |> put_resp_content_type(mime)
    |> send_file(:ok, path, 0, size)
  end

  defp serve(conn, path, mime, size, "bytes=" <> range) do
    case parse_range(range, size) do
      {:ok, first, last} ->
        length = last - first + 1

        conn
        |> put_resp_header("accept-ranges", "bytes")
        |> put_resp_header("content-range", "bytes #{first}-#{last}/#{size}")
        |> put_resp_header("content-length", Integer.to_string(length))
        |> put_resp_content_type(mime)
        |> send_file(:partial_content, path, first, length)

      :error ->
        range_not_satisfiable(conn, size)
    end
  end

  defp serve(conn, _path, _mime, size, _range), do: range_not_satisfiable(conn, size)

  defp parse_range(_range, size) when size <= 0, do: :error

  defp parse_range(range, size) when is_binary(range) do
    if Regex.match?(@range_pattern, range) do
      parse_valid_range(range, size)
    else
      :error
    end
  end

  defp parse_valid_range(range, size) do
    case String.split(range, "-", parts: 2) do
      ["", suffix] ->
        with {count, ""} when count > 0 <- Integer.parse(suffix) do
          length = min(count, size)
          {:ok, size - length, size - 1}
        else
          _ -> :error
        end

      [first, ""] ->
        with {start, ""} when start < size and start >= 0 <- Integer.parse(first) do
          {:ok, start, size - 1}
        else
          _ -> :error
        end

      [first, last] ->
        with {start, ""} when start >= 0 and start < size <- Integer.parse(first),
             {finish, ""} when finish >= start <- Integer.parse(last) do
          {:ok, start, min(finish, size - 1)}
        else
          _ -> :error
        end

      _ ->
        :error
    end
  end

  defp range_not_satisfiable(conn, size) do
    conn
    |> put_resp_header("content-range", "bytes */#{size}")
    |> put_resp_header("accept-ranges", "bytes")
    |> send_resp(:requested_range_not_satisfiable, "")
  end
end
