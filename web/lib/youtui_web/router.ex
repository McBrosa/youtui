defmodule YoutuiWeb.Router do
  use YoutuiWeb, :router

  pipeline :browser do
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {YoutuiWeb.Layouts, :root}
    plug :protect_from_forgery
    plug :put_secure_browser_headers
  end

  pipeline :api do
    plug :accepts, ["json"]
  end

  pipeline :authenticated_browser do
    plug YoutuiWeb.Plugs.RequireSession
  end

  pipeline :authenticated_media do
    plug :fetch_session
    plug YoutuiWeb.Plugs.RequireSession
  end

  scope "/", YoutuiWeb do
    pipe_through :api

    get "/health", HealthController, :show
  end

  scope "/", YoutuiWeb do
    pipe_through :browser

    get "/start", SessionController, :start
  end

  scope "/", YoutuiWeb do
    pipe_through [:browser, :authenticated_browser]

    live "/", PlayerLive, :index
  end

  scope "/", YoutuiWeb do
    pipe_through :authenticated_media

    get "/media/:media_id", MediaController, :show
    head "/media/:media_id", MediaController, :show
    get "/media/:media_id/:kind/index.m3u8", MediaController, :rendition
    get "/media/:media_id/:kind/:index", MediaController, :asset
  end

  # Other scopes may use custom stacks.
  # scope "/api", YoutuiWeb do
  #   pipe_through :api
  # end
end
