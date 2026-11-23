defmodule Youtui.MediaHTTP do
  @moduledoc false

  @timeout 15_000
  @redirects 3
  # Signed media URLs must never inherit application-wide Req authentication or query options.
  @client Req.Request.new() |> Req.Steps.attach()

  def fetch(url, limit, range \\ nil)

  def fetch(url, limit, range) when is_integer(limit) and limit > 0 do
    with :ok <- Youtui.Media.check_cdn_url(url), {:ok, headers} <- range_headers(range) do
      # Bound the complete fetch, including DNS, redirects and a slowly trickling body.
      task = Task.async(fn -> request(url, limit, headers, @redirects) end)

      case Task.yield(task, @timeout) || Task.shutdown(task, :brutal_kill) do
        {:ok, result} -> result
        _ -> {:error, :media_fetch_failed}
      end
    else
      :error -> {:error, :media_fetch_failed}
      {:error, :invalid_range} = error -> error
    end
  end

  def fetch(_, _, _), do: {:error, :media_fetch_failed}

  defp range_headers(nil), do: {:ok, []}

  defp range_headers(range) when is_binary(range) and byte_size(range) <= 64 do
    if Regex.match?(~r/\Abytes=(?:[0-9]+-[0-9]*|-[0-9]+)\z/, range),
      do: {:ok, [{"range", range}]},
      else: {:error, :invalid_range}
  end

  defp range_headers(_), do: {:error, :invalid_range}

  defp request(url, limit, headers, redirects) do
    options =
      Application.get_env(:youtui_web, :media_http_options, [])
      |> Keyword.take([:plug, :adapter])
      |> Keyword.merge(
        url: url,
        headers: [{"accept-encoding", "identity"} | headers],
        redirect: false,
        retry: false,
        retry_log_level: false,
        redirect_log_level: false,
        raw: true,
        connect_options: [timeout: 4_000],
        receive_timeout: 5_000,
        request_timeout: @timeout,
        into: &collect(&1, &2, limit)
      )

    case Req.get(@client, options) do
      {:ok, %{body: :too_large}} ->
        {:error, :body_too_large}

      {:ok, %{status: status} = response} when status in [200, 206] ->
        {:ok,
         %{
           url: url,
           status: status,
           body: IO.iodata_to_binary(response.body),
           content_type: header(response, "content-type"),
           content_range: header(response, "content-range")
         }}

      {:ok, %{status: status} = response}
      when status in [301, 302, 303, 307, 308] and redirects > 0 ->
        with [location] <- Req.Response.get_header(response, "location"),
             {:ok, next} <- Youtui.Media.resolve_cdn_url(url, location) do
          request(next, limit, headers, redirects - 1)
        else
          _ -> {:error, :media_fetch_failed}
        end

      {:ok, %{status: 416}} ->
        {:error, :invalid_range}

      _ ->
        {:error, :media_fetch_failed}
    end
  rescue
    # Pool checkout can raise; never return exceptions containing signed request URLs.
    _ in [ArgumentError, RuntimeError] -> {:error, :media_fetch_failed}
  end

  defp collect({:data, chunk}, {req, %{status: status} = resp}, limit)
       when status in [200, 206] do
    size = Req.Response.get_private(resp, :media_bytes, 0) + byte_size(chunk)

    if size > limit do
      {:halt, {req, %{resp | body: :too_large}}}
    else
      resp = Req.Response.put_private(resp, :media_bytes, size)
      {:cont, {req, %{resp | body: [resp.body, chunk]}}}
    end
  end

  defp collect({:data, _}, request_response, _limit), do: {:halt, request_response}

  defp header(response, name) do
    case Req.Response.get_header(response, name) do
      [value] -> value
      _ -> nil
    end
  end
end
