defmodule YoutuiWeb.HealthController do
  use YoutuiWeb, :controller

  def show(conn, _params) do
    cond do
      not YoutuiWeb.Auth.health_authorized?(conn) ->
        conn |> put_status(:unauthorized) |> json(%{ready: false})

      Youtui.Backend.ready?() ->
        json(conn, %{ready: true})

      true ->
        conn |> put_status(:service_unavailable) |> json(%{ready: false})
    end
  end
end
