defmodule Youtui.HLS do
  @moduledoc "Rewrites finite CDN media playlists to locally registered, same-origin resources."

  alias Youtui.{Media, MediaHTTP}

  @playlist_bytes 8 * 1024 * 1024
  @retained_bytes 2 * 1024 * 1024
  @max_assets 4096
  @max_lines 32_768
  @plain_tags ~w(#EXTM3U #EXTINF #EXT-X-VERSION #EXT-X-TARGETDURATION #EXT-X-MEDIA-SEQUENCE
    #EXT-X-DISCONTINUITY-SEQUENCE #EXT-X-DISCONTINUITY #EXT-X-PROGRAM-DATE-TIME
    #EXT-X-BYTERANGE #EXT-X-PLAYLIST-TYPE #EXT-X-ALLOW-CACHE #EXT-X-INDEPENDENT-SEGMENTS
    #EXT-X-START #EXT-X-GAP #EXT-X-ENDLIST)

  def variant_attributes(media) do
    video = media["video_codec"]
    audio = media["audio_codec"]

    codecs =
      if is_binary(video) and Regex.match?(~r/\Aavc1\.[0-9A-Fa-f]{6}\z/, video) and
           is_binary(audio) and Regex.match?(~r/\Amp4a\.40\.\d{1,3}\z/, audio),
         do: ~s(,CODECS="#{video},#{audio}"),
         else: ""

    resolution =
      case {media["width"], media["height"]} do
        {w, h} when is_integer(w) and w in 1..16_384 and is_integer(h) and h in 1..720 ->
          ",RESOLUTION=#{w}x#{h}"

        _ ->
          ""
      end

    codecs <> resolution
  end

  def rendition(id, kind) do
    case Media.rendition(id, kind) do
      {:fetch, url} ->
        with {:ok, %{status: 200, body: body, url: final_url}} <-
               MediaHTTP.fetch(url, @playlist_bytes),
             {:ok, rendition} <- rewrite(body, final_url) do
          Media.store_rendition(id, kind, url, rendition)
        else
          _ -> {:error, :bad_playlist}
        end

      result ->
        result
    end
  end

  defp rewrite(body, base) do
    with true <- byte_size(body) <= @playlist_bytes and String.valid?(body),
         false <- Regex.match?(~r/[\x00-\x09\x0b\x0c\x0e-\x1f\x7f]|\r(?!\n)/, body),
         ["#EXTM3U" | _] = lines <- String.split(body, ["\r\n", "\n"], parts: @max_lines + 1),
         true <- length(lines) <= @max_lines and "#EXT-X-ENDLIST" in lines,
         {lines, [first | _] = assets, count, bytes} <-
           Enum.reduce_while(lines, {[], [], 0, 0}, &rewrite_line(&1, &2, base)),
         prefix_size = :binary.longest_common_prefix(assets),
         true <- bytes - (count - 1) * prefix_size <= @retained_bytes,
         playlist = lines |> Enum.reverse() |> Enum.join("\n"),
         true <- byte_size(playlist) <= @retained_bytes do
      # Signed segments often share a kilobyte of URL. Copy slices so the registry
      # can release the original manifest while keeping each URL byte-for-byte exact.
      prefix = :binary.copy(binary_part(first, 0, prefix_size))

      suffixes =
        Enum.reduce(assets, [], fn url, acc ->
          suffix = binary_part(url, prefix_size, byte_size(url) - prefix_size)
          [:binary.copy(suffix) | acc]
        end)

      {:ok, %{playlist: playlist, url_prefix: prefix, url_suffixes: List.to_tuple(suffixes)}}
    else
      _ -> :error
    end
  end

  defp rewrite_line(line, {lines, assets, count, bytes} = acc, base) do
    case reference(line) do
      :keep ->
        {:cont, {[line | lines], assets, count, bytes}}

      :drop ->
        {:cont, acc}

      {:asset, ref} when count < @max_assets ->
        with {:ok, url} <- Media.resolve_cdn_url(base, ref),
             size when size <= @playlist_bytes <- bytes + byte_size(url) do
          index = Integer.to_string(count)

          rewritten =
            if line == ref,
              do: index,
              else: String.replace(line, ~s(URI="#{ref}"), ~s(URI="#{index}"))

          {:cont, {[rewritten | lines], [url | assets], count + 1, size}}
        else
          _ -> {:halt, :error}
        end

      _ ->
        {:halt, :error}
    end
  end

  defp reference(""), do: :keep
  defp reference("#EXT-X-KEY:METHOD=NONE"), do: :keep

  defp reference("#" <> _ = line) do
    case line |> :binary.split(":") |> hd() do
      tag when tag in ["#EXT-X-KEY", "#EXT-X-MAP"] ->
        with [_] <- :binary.matches(line, "URI="),
             [url] <- Regex.run(~r/[:,]URI="([^"]+)"(?:,|$)/, line, capture: :all_but_first) do
          {:asset, url}
        else
          _ -> :error
        end

      tag when tag in @plain_tags ->
        :keep

      "#EXT" <> _ ->
        :error

      _ ->
        :drop
    end
  end

  defp reference(url), do: {:asset, url}
end
