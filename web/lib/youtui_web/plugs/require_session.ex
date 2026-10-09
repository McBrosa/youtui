defmodule YoutuiWeb.Plugs.RequireSession do
  @moduledoc false
  import Plug.Conn

  def init(opts), do: opts

  def call(conn, _opts) do
    if YoutuiWeb.Auth.authenticated?(conn) do
      conn
    else
      conn
      |> put_resp_content_type("text/plain")
      |> send_resp(:unauthorized, "authentication required")
      |> halt()
    end
  end
end
