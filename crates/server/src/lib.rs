//! HTTP server: Slack Web API, browser session auth, and the embedded web UI.

pub mod api;
pub mod auth;
mod config;
pub mod delivery;
pub mod events;
pub mod harness;
pub mod interactions;
pub mod ratelimit;
pub mod services;
mod socket_mode;
mod web;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use slack_emu_core::ts::TsClock;
use slack_emu_store::{PgStore, SqliteStore, Store};
use tower_http::trace::TraceLayer;

use crate::delivery::Delivery;
use crate::events::EventBus;
use crate::harness::ApiLog;
use crate::interactions::Interactions;
use crate::ratelimit::RateLimiter;

pub use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<dyn Store>,
    /// Issues message timestamps.
    pub clock: Arc<TsClock>,
    pub bus: EventBus,
    pub delivery: Arc<Delivery>,
    pub interactions: Arc<Interactions>,
    /// Recent Web API calls, for the harness and inspector.
    pub api_log: Arc<ApiLog>,
    pub rate_limits: Arc<RateLimiter>,
}

impl AppState {
    /// Connects the store and makes sure the default workspace (and every
    /// workspace's default channels) exist.
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let url = &config.database_url;
        let store: Arc<dyn Store> = if url.starts_with("sqlite:") {
            Arc::new(SqliteStore::connect(url).await?)
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Arc::new(PgStore::connect(url).await?)
        } else {
            anyhow::bail!("DATABASE_URL must start with sqlite:, postgres:// or postgresql://");
        };
        let clock = Arc::new(TsClock::starting_after(store.max_ts().await?.as_deref()));
        let delivery = Arc::new(Delivery::new(config.event_retry_delays.clone()));
        let config_rate_limits = config.rate_limits;
        let state = Self {
            config: Arc::new(config),
            store,
            clock,
            bus: EventBus::default(),
            delivery,
            interactions: Arc::default(),
            api_log: Arc::default(),
            rate_limits: Arc::new(RateLimiter::new(config_rate_limits)),
        };
        delivery::spawn_dispatcher(state.clone());
        state
            .ensure_default_workspace()
            .await
            .map_err(|e| anyhow::anyhow!("setting up workspaces: {}", e.code))?;
        if let Some(path) = state.config.seed_file.clone() {
            harness::seed_file_on_empty(&state, &path).await?;
        }
        Ok(state)
    }

    /// Creates the default workspace if it's missing, and every workspace's default channels.
    pub async fn ensure_default_workspace(&self) -> api::ApiResult<()> {
        let cfg = &self.config;
        if self
            .store
            .get_workspace_by_domain(&cfg.default_workspace_domain)
            .await?
            .is_none()
        {
            let ws = self
                .store
                .create_workspace(&cfg.default_workspace_name, &cfg.default_workspace_domain)
                .await?;
            tracing::info!("created default workspace {} ({})", ws.name, ws.id);
        }
        for ws in self.store.list_workspaces().await? {
            services::ensure_workspace_defaults(self, &ws.id).await?;
        }
        Ok(())
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/auth", auth::routes())
        .nest("/api", api::routes())
        .nest("/ws", ws::routes().merge(socket_mode::routes()))
        .nest("/hooks", interactions::routes())
        .merge(api::files::routes(&state))
        .nest("/_emu", harness::routes(state.clone()))
        .route("/archives/{channel}/{p}", get(permalink))
        .fallback(web::static_handler)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Slack-style permalinks (`/archives/C123/p1730000000000100`) redirect into the client.
async fn permalink(State(state): State<AppState>, Path((channel, p)): Path<(String, String)>) -> Response {
    match api::resolve_permalink(&state, &channel, &p).await {
        Ok(Some((team, ts))) => Redirect::to(&format!("/client/{team}/{channel}?ts={ts}")).into_response(),
        Ok(None) => Redirect::to("/").into_response(),
        Err(e) => e.into_response(),
    }
}
