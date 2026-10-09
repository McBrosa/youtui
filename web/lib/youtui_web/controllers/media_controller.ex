defmodule YoutuiWeb.MediaController do
  use YoutuiWeb, :controller

  @range_pattern ~r/\A[0-9]*-[0-9]*\z/

  def show(conn, %{"media_id" => media_id}) do
    with {:ok, %{path: path, mime: mime}} <- Youtui.Media.lookup(media_id),
         {:ok, %{size: size}} <- File.stat(path) do
      serve(conn, path, mime, size, get_req_header(conn, "range") |> List.first())
    else
      _ -> send_resp(conn, :not_found, "media not found")
    end
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
