defmodule YoutuiWeb.SessionController do
  use YoutuiWeb, :controller

  def start(conn, %{"token" => token}) do
    case YoutuiWeb.Auth.exchange(token) do
      {:ok, session} ->
        conn
        |> configure_session(renew: true)
        |> put_session(:youtui_session, session)
        |> put_resp_header("cache-control", "no-store")
        |> put_resp_header("referrer-policy", "no-referrer")
        |> redirect(to: "/")

      :error ->
        conn
        |> no_store()
        |> put_status(:unauthorized)
        |> text("invalid or expired launch token")
    end
  end

  def start(conn, _params) do
    conn
    |> no_store()
    |> put_status(:unauthorized)
    |> text("launch token required")
  end

  defp no_store(conn) do
    conn
    |> put_resp_header("cache-control", "no-store")
    |> put_resp_header("referrer-policy", "no-referrer")
  end
end
