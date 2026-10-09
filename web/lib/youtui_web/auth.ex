defmodule YoutuiWeb.Auth do
  @moduledoc false

  @session_key :youtui_session

  def exchange(supplied) when is_binary(supplied) do
    expected = System.get_env("YOUTUI_WEB_TOKEN")

    if is_binary(expected) and expected != "" and byte_size(expected) == byte_size(supplied) and
         Plug.Crypto.secure_compare(expected, supplied) do
      session = :crypto.strong_rand_bytes(32) |> Base.url_encode64(padding: false)
      {:ok, session}
    else
      :error
    end
  end

  def exchange(_), do: :error

  def authenticated?(conn), do: is_binary(Plug.Conn.get_session(conn, @session_key))

  def health_authorized?(conn) do
    supplied = Plug.Conn.get_req_header(conn, "x-youtui-token") |> List.first()
    expected = System.get_env("YOUTUI_WEB_TOKEN")

    is_binary(supplied) and is_binary(expected) and expected != "" and
      byte_size(supplied) == byte_size(expected) and
      Plug.Crypto.secure_compare(supplied, expected)
  end
end
