//! The two pretend repos: their files at `main`, their history and the branches agents and teammates have open.
//! A branch's changes are written as edits to the files at `main` (or as new files), so the diffs the apps show
//! come out of the same text the file viewer shows.

/// One file a branch touches: edits to what `main` has (each `(old, new)` replaces the text `old`), or a new file.
pub struct Ch {
    pub path: &'static str,
    pub edits: &'static [(&'static str, &'static str)],
    pub new: Option<&'static str>,
}

/// A commit on `main`, or the one that opens a line of work: `key` names it, `parents` are keys.
pub struct Cm {
    pub key: &'static str,
    pub parents: &'static [&'static str],
    /// hours ago
    pub age: f64,
    /// 0 is you, 1 the teammate
    pub who: u8,
    pub subject: &'static str,
}

/// A pull request on GitHub: number, title, author login, draft, review decision, checks as (name, conclusion).
pub struct Pr {
    pub number: u64,
    pub title: &'static str,
    pub author: &'static str,
    pub draft: bool,
    pub review: &'static str,
    pub checks: &'static [(&'static str, &'static str)],
    /// hours since it was touched
    pub updated: f64,
}

/// A branch off `main` with work on it.
pub struct Line {
    pub branch: &'static str,
    /// the key of the `main` commit it forked from
    pub fork: &'static str,
    /// its commits, oldest first: subject and hours ago
    pub commits: &'static [(&'static str, f64)],
    pub changes: &'static [Ch],
    /// paths in `changes` not committed yet (only for a worktree)
    pub dirty: &'static [&'static str],
    /// an agent works in a worktree on it
    pub worktree: bool,
    pub local: bool,
    pub remote: bool,
    /// `main` already has it (it was merged), so `main`'s files show the changes
    pub in_main: bool,
    pub pr: Option<Pr>,
}

/// One repo.
pub struct Rp {
    pub name: &'static str,
    pub remote: &'static str,
    pub slug: &'static str,
    pub people: [(&'static str, &'static str); 2],
    pub files: &'static [(&'static str, &'static str)],
    pub history: &'static [Cm],
    /// the key of the commit `origin/main` is at
    pub origin: &'static str,
    /// edited in the main checkout, not committed
    pub dirty: &'static [Ch],
    pub lines: &'static [Line],
    /// what an agent started at runtime does, in a worktree (its `branch` is blank)
    pub generic: Line,
}

pub static REPOS: [Rp; 2] = [SHOP, SPHERE];

// ------------------------------------------------------------------ shop-api

const SHOP: Rp = Rp {
    name: "shop-api",
    remote: "git@github.com:you/shop-api.git",
    slug: "you/shop-api",
    people: [("You", "you@example.com"), ("Amara Okafor", "amara@example.com")],
    files: SHOP_FILES,
    history: SHOP_HISTORY,
    origin: "p15",
    dirty: &[],
    lines: SHOP_LINES,
    generic: Line {
        branch: "",
        fork: "p16",
        commits: &[("Add /ready, a readiness check that asks the database", 0.2), ("Test /health and /ready", 0.1)],
        changes: &[
            Ch { path: "src/routes/health.rs", edits: HEALTH_READY, new: None },
            Ch { path: "tests/health.rs", edits: &[], new: Some(HEALTH_TEST) },
        ],
        dirty: &[],
        worktree: true,
        local: true,
        remote: false,
        in_main: false,
        pr: None,
    },
};

const SHOP_HISTORY: &[Cm] = &[
    Cm { key: "p01", parents: &[], age: 312.0, who: 0, subject: "Initial commit" },
    Cm { key: "p02", parents: &["p01"], age: 306.0, who: 0, subject: "Scaffold the axum service with /health" },
    Cm { key: "p03", parents: &["p02"], age: 290.0, who: 0, subject: "Add the Postgres pool and the first migration" },
    Cm { key: "p04", parents: &["p03"], age: 262.0, who: 1, subject: "Products: list and fetch" },
    Cm { key: "p05", parents: &["p04"], age: 240.0, who: 0, subject: "Add login and token issuing" },
    Cm { key: "p06", parents: &["p05"], age: 214.0, who: 1, subject: "Orders: create, list and fetch" },
    Cm { key: "p07", parents: &["p06"], age: 190.0, who: 0, subject: "Cache product reads in Redis" },
    Cm { key: "p08", parents: &["p07"], age: 168.0, who: 0, subject: "Add mailpit and the worker to compose" },
    Cm { key: "p09", parents: &["p08"], age: 143.0, who: 1, subject: "Send order receipts through the job queue" },
    Cm { key: "p10", parents: &["p09"], age: 120.0, who: 0, subject: "Integration tests for login" },
    Cm { key: "p11", parents: &["p10"], age: 96.0, who: 1, subject: "Rate limit POST /auth/login" },
    Cm { key: "p12", parents: &["p11"], age: 74.0, who: 0, subject: "docs: first pass at api.md" },
    Cm { key: "p13", parents: &["p12"], age: 60.0, who: 1, subject: "Fix sqlx offline data after the migration rename" },
    Cm { key: "p14", parents: &["p13"], age: 49.0, who: 1, subject: "Return 404 for unknown orders instead of 500" },
    Cm { key: "m1", parents: &["p14", "sphere/bump-postgres-to-16-4#0"], age: 46.0, who: 0, subject: "Merge branch 'sphere/bump-postgres-to-16-4'" },
    Cm { key: "p15", parents: &["m1"], age: 22.0, who: 1, subject: "Log request ids in the http span" },
    Cm { key: "p16", parents: &["p15"], age: 5.0, who: 0, subject: "Tidy order totals rounding" },
];

const SHOP_LINES: &[Line] = &[
    Line {
        branch: "sphere/fix-flaky-login-test",
        fork: "p15",
        commits: &[("Wait for the server instead of sleeping in the login tests", 3.0)],
        changes: &[
            Ch { path: "src/auth/token.rs", edits: TOKEN_FIX, new: None },
            Ch { path: "tests/login.rs", edits: LOGIN_TEST_FIX, new: None },
            Ch { path: "tests/support/mod.rs", edits: &[], new: Some(SUPPORT_MOD) },
        ],
        dirty: &["tests/login.rs"],
        worktree: true,
        local: true,
        remote: false,
        in_main: false,
        pr: None,
    },
    Line {
        branch: "sphere/reply-to-the-api-review",
        fork: "p16",
        commits: &[("Rename the order handlers and paginate GET /orders", 1.5), ("Document the pagination parameters", 1.2)],
        changes: &[
            Ch { path: "docs/api.md", edits: API_DOC_PAGES, new: None },
            Ch { path: "src/routes/orders.rs", edits: ORDERS_PAGES, new: None },
            Ch { path: "tests/orders_api.rs", edits: &[], new: Some(ORDERS_TEST) },
        ],
        dirty: &[],
        worktree: true,
        local: true,
        remote: true,
        in_main: false,
        pr: Some(Pr { number: 31, title: "Reply to the API review: paginate GET /orders", author: "you", draft: false, review: "REVIEW_REQUIRED", checks: &[("fmt", "SUCCESS"), ("clippy", "SUCCESS"), ("test", "SUCCESS")], updated: 1.1 }),
    },
    Line {
        branch: "sphere/bump-postgres-to-16-4",
        fork: "p13",
        commits: &[("Bump postgres to 16.4 in compose and CI", 52.0)],
        changes: &[Ch { path: ".github/workflows/ci.yml", edits: &[("image: postgres:16.1", "image: postgres:16.4")], new: None }, Ch { path: "docker-compose.yml", edits: &[("image: postgres:16.1", "image: postgres:16.4")], new: None }],
        dirty: &[],
        worktree: true,
        local: true,
        remote: false,
        in_main: true,
        pr: None,
    },
    Line {
        branch: "feature/refunds",
        fork: "p11",
        commits: &[("Add the refunds table", 70.0), ("POST /orders/:id/refund", 40.0)],
        changes: &[Ch { path: "migrations/0003_refunds.sql", edits: &[], new: Some(REFUND_SQL) }, Ch { path: "src/routes/orders.rs", edits: ORDERS_REFUND, new: None }],
        dirty: &[],
        worktree: false,
        local: true,
        remote: true,
        in_main: false,
        pr: Some(Pr { number: 29, title: "Refunds: table and POST /orders/:id/refund", author: "you", draft: true, review: "", checks: &[("fmt", "SUCCESS"), ("clippy", "SUCCESS"), ("test", "IN_PROGRESS")], updated: 38.0 }),
    },
    Line {
        branch: "amara/order-export-csv",
        fork: "m1",
        commits: &[("Export orders as CSV", 18.0)],
        changes: &[Ch { path: "Cargo.toml", edits: &[("jsonwebtoken = \"9\"\n", "jsonwebtoken = \"9\"\ncsv = \"1.3\"\n")], new: None }, Ch { path: "src/routes/orders.rs", edits: ORDERS_CSV, new: None }],
        dirty: &[],
        worktree: false,
        local: false,
        remote: true,
        in_main: false,
        pr: Some(Pr { number: 27, title: "Export orders as CSV", author: "amara-okafor", draft: false, review: "CHANGES_REQUESTED", checks: &[("fmt", "SUCCESS"), ("clippy", "SUCCESS"), ("test", "FAILURE")], updated: 16.0 }),
    },
];

const SHOP_FILES: &[(&str, &str)] = &[
    (".gitignore", "/target\n.env\n*.log\n"),
    ("Cargo.toml", SHOP_CARGO),
    ("Dockerfile", SHOP_DOCKERFILE),
    ("README.md", SHOP_README),
    ("docker-compose.yml", SHOP_COMPOSE),
    (".github/workflows/ci.yml", SHOP_CI),
    ("docs/api.md", SHOP_API_DOC),
    ("migrations/0001_init.sql", SHOP_MIG1),
    ("migrations/0002_orders.sql", SHOP_MIG2),
    ("src/lib.rs", SHOP_LIB),
    ("src/main.rs", SHOP_MAIN),
    ("src/config.rs", SHOP_CONFIG),
    ("src/db.rs", SHOP_DB),
    ("src/auth/mod.rs", SHOP_AUTH_MOD),
    ("src/auth/token.rs", SHOP_TOKEN),
    ("src/routes/mod.rs", SHOP_ROUTES),
    ("src/routes/health.rs", SHOP_HEALTH),
    ("src/routes/login.rs", SHOP_LOGIN),
    ("src/routes/orders.rs", SHOP_ORDERS),
    ("src/routes/products.rs", SHOP_PRODUCTS),
    ("tests/login.rs", SHOP_LOGIN_TEST),
];

const SHOP_CARGO: &str = r##"[package]
name = "shop-api"
version = "0.4.2"
edition = "2021"
rust-version = "1.80"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "time"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "migrate", "chrono"] }
redis = { version = "0.27", features = ["tokio-comp", "connection-manager"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "1"
futures = "0.3"
tower-http = { version = "0.6", features = ["trace"] }
jsonwebtoken = "9"

[dev-dependencies]
reqwest = { version = "0.12", features = ["json"] }
"##;

const SHOP_DOCKERFILE: &str = r##"FROM rust:1.82-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=build /app/target/release/shop-api /usr/local/bin/shop-api
EXPOSE 8080
CMD ["shop-api"]
"##;

const SHOP_README: &str = r##"# shop-api

A small web service for a shop: products, orders and logins. Rust, axum, Postgres and Redis.

## Run it

    docker compose up -d db cache mail
    cargo run

The API listens on http://localhost:3000 when run with cargo and on :8080 inside compose.
Mailpit catches outgoing mail at http://localhost:8025.

## Test it

    cargo test

The integration tests use `sqlx::test`, which needs `DATABASE_URL` to point at the compose database.

See [docs/api.md](docs/api.md) for the endpoints.
"##;

const SHOP_COMPOSE: &str = r##"name: shop
services:
  api:
    build: .
    image: shop-api:dev
    ports:
      - "8080:8080"
    environment:
      DATABASE_URL: postgres://shop:shop@db:5432/shop
      REDIS_URL: redis://cache:6379/0
    depends_on:
      db:
        condition: service_healthy
      cache:
        condition: service_started
  db:
    image: postgres:16.4
    environment:
      POSTGRES_USER: shop
      POSTGRES_PASSWORD: shop
      POSTGRES_DB: shop
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U shop"]
      interval: 5s
      retries: 10
  cache:
    image: redis:7-alpine
    ports:
      - "6379:6379"
  mail:
    image: axllent/mailpit
    ports:
      - "8025:8025"
      - "1025:1025"
  worker:
    image: shop-worker:dev
    command: ["shop-worker"]
    depends_on:
      - cache
"##;

const SHOP_CI: &str = r##"name: ci
on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16.4
        env:
          POSTGRES_USER: shop
          POSTGRES_PASSWORD: shop
          POSTGRES_DB: shop
        ports: ["5432:5432"]
        options: >-
          --health-cmd "pg_isready -U shop"
          --health-interval 5s
          --health-retries 10
      redis:
        image: redis:7-alpine
        ports: ["6379:6379"]
    env:
      DATABASE_URL: postgres://shop:shop@localhost:5432/shop
      REDIS_URL: redis://localhost:6379/0
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
"##;

const SHOP_API_DOC: &str = r##"# Shop API

Base URL in development: `http://localhost:8080`. Bodies are JSON.

## Auth

### POST /auth/login

Body: `{"email": "...", "password": "..."}`

Returns `{"token": "..."}`. Send it as `Authorization: Bearer <token>`. Tokens last 15 minutes.
Five failed logins from one address in a minute are answered with `429`.

## Products

### GET /products

Query: `limit` (default 20, at most 100).

### GET /products/{id}

Returns one product, or `404`.

## Orders

### GET /orders

Returns the caller's orders, newest first.
At most 50 are returned.

### GET /orders/{id}

Returns one order, or `404`.

### POST /orders

Body: `{"items": [{"product_id": 1, "quantity": 2}]}`. Returns `201` and the order.
A receipt is emailed through the job queue.
"##;

const SHOP_MIG1: &str = r##"CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE products (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    price_cents BIGINT NOT NULL CHECK (price_cents >= 0),
    stock INT NOT NULL DEFAULT 0
);
"##;

const SHOP_MIG2: &str = r##"CREATE TABLE orders (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users (id),
    total_cents BIGINT NOT NULL,
    status TEXT NOT NULL DEFAULT 'new',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE order_items (
    order_id BIGINT NOT NULL REFERENCES orders (id) ON DELETE CASCADE,
    product_id BIGINT NOT NULL REFERENCES products (id),
    quantity INT NOT NULL CHECK (quantity > 0),
    PRIMARY KEY (order_id, product_id)
);
"##;

const SHOP_LIB: &str = r##"pub mod auth;
pub mod config;
pub mod db;
pub mod routes;

use axum::Router;
use tower_http::trace::TraceLayer;

/// Everything a handler needs.
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub cache: redis::aio::ConnectionManager,
    pub config: config::Config,
}

impl AppState {
    /// State for the integration tests: the pool `sqlx::test` hands over, and the Redis from compose.
    pub fn for_tests(pool: sqlx::PgPool) -> AppState {
        let config = config::Config::from_env();
        let client = redis::Client::open(config.redis_url.as_str()).expect("a redis url");
        let cache = futures::executor::block_on(client.get_connection_manager()).expect("redis for the tests");
        AppState { db: pool, cache, config }
    }
}

pub fn build_app(state: AppState) -> Router {
    routes::router().layer(TraceLayer::new_for_http()).with_state(state)
}
"##;

const SHOP_MAIN: &str = r##"use shop_api::{build_app, config::Config, db, AppState};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let config = Config::from_env();
    let pool = db::connect(&config.database_url).await?;
    sqlx::migrate!().run(&pool).await?;
    let cache = redis::Client::open(config.redis_url.as_str())?.get_connection_manager().await?;
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    info!("listening on {}", config.listen);
    axum::serve(listener, build_app(AppState { db: pool, cache, config })).await?;
    Ok(())
}
"##;

const SHOP_CONFIG: &str = r##"/// Settings, read from the environment.
#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    pub listen: String,
    pub token_secret: String,
}

impl Config {
    pub fn from_env() -> Config {
        let var = |k: &str, default: &str| std::env::var(k).unwrap_or_else(|_| default.to_string());
        Config {
            database_url: var("DATABASE_URL", "postgres://shop:shop@localhost:5432/shop"),
            redis_url: var("REDIS_URL", "redis://localhost:6379/0"),
            listen: var("LISTEN", "0.0.0.0:8080"),
            token_secret: var("TOKEN_SECRET", "dev-only-secret"),
        }
    }
}
"##;

const SHOP_DB: &str = r##"use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(10).min_connections(2).connect(url).await
}
"##;

const SHOP_AUTH_MOD: &str = r##"pub mod token;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    Invalid,
    #[error("token expired")]
    Expired,
    #[error("internal error")]
    Internal,
}
"##;

const SHOP_TOKEN: &str = r##"use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use super::AuthError;

pub const TTL_SECS: u64 = 900;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,
    pub exp: u64,
}

pub fn issue(user_id: i64, secret: &str, now: u64) -> Result<String, AuthError> {
    let claims = Claims { sub: user_id, exp: now + TTL_SECS };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes())).map_err(|_| AuthError::Internal)
}

pub fn verify(token: &str, secret: &str, now: u64) -> Result<Claims, AuthError> {
    let mut validation = Validation::default();
    // expiry is checked below, against the caller's clock
    validation.validate_exp = false;
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation).map_err(|_| AuthError::Invalid)?;
    if data.claims.exp < now {
        return Err(AuthError::Expired);
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_token_verifies() {
        let t = issue(7, "s3cret", 1_000).unwrap();
        assert_eq!(verify(&t, "s3cret", 1_001).unwrap().sub, 7);
    }

    #[test]
    fn a_token_expires_when_its_time_is_up() {
        let t = issue(7, "s3cret", 1_000).unwrap();
        assert!(verify(&t, "s3cret", 1_000 + TTL_SECS).is_ok());
        assert!(matches!(verify(&t, "s3cret", 1_000 + TTL_SECS + 1), Err(AuthError::Expired)));
    }
}
"##;

const SHOP_ROUTES: &str = r##"use axum::Router;

use crate::AppState;

pub mod health;
pub mod login;
pub mod orders;
pub mod products;

pub fn router() -> Router<AppState> {
    Router::new().merge(health::routes()).merge(login::routes()).merge(products::routes()).merge(orders::routes())
}
"##;

const SHOP_HEALTH: &str = r##"use axum::{routing::get, Router};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

async fn health() -> &'static str {
    "ok"
}
"##;

const SHOP_LOGIN: &str = r##"use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use crate::{auth::token, AppState};

#[derive(Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct Session {
    pub token: String,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/auth/login", post(login))
}

async fn login(State(state): State<AppState>, Json(body): Json<Credentials>) -> Result<Json<Session>, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, password_hash FROM users WHERE email = $1")
        .bind(&body.email)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))))?;
    match row {
        Some((id, hash)) if hash == body.password => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let token = token::issue(id, &state.config.token_secret, now).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))))?;
            Ok(Json(Session { token }))
        }
        _ => Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "invalid credentials"})))),
    }
}
"##;

const SHOP_PRODUCTS: &str = r##"use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::AppState;

#[derive(Debug, Serialize, FromRow)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub price_cents: i64,
    pub stock: i32,
}

#[derive(Debug, Deserialize)]
pub struct Limit {
    pub limit: Option<i64>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/products", get(list)).route("/products/:id", get(get_one))
}

async fn list(State(state): State<AppState>, Query(q): Query<Limit>) -> Result<Json<Vec<Product>>, StatusCode> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    sqlx::query_as::<_, Product>("SELECT id, name, price_cents, stock FROM products ORDER BY id LIMIT $1")
        .bind(limit)
        .fetch_all(&state.db)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Product>, StatusCode> {
    sqlx::query_as::<_, Product>("SELECT id, name, price_cents, stock FROM products WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
"##;

const SHOP_ORDERS: &str = r##"use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::AppState;

#[derive(Debug, Serialize, FromRow)]
pub struct Order {
    pub id: i64,
    pub user_id: i64,
    pub total_cents: i64,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct NewOrder {
    pub items: Vec<NewItem>,
}

#[derive(Debug, Deserialize)]
pub struct NewItem {
    pub product_id: i64,
    pub quantity: i32,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orders", get(list).post(create))
        .route("/orders/:id", get(get_one))
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Order>>, StatusCode> {
    let orders = sqlx::query_as::<_, Order>("SELECT id, user_id, total_cents, status FROM orders ORDER BY id DESC LIMIT 50")
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(orders))
}

async fn get_one(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Order>, StatusCode> {
    sqlx::query_as::<_, Order>("SELECT id, user_id, total_cents, status FROM orders WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn create(State(state): State<AppState>, Json(body): Json<NewOrder>) -> Result<(StatusCode, Json<Order>), StatusCode> {
    let mut tx = state.db.begin().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut total = 0i64;
    for item in &body.items {
        let price: i64 = sqlx::query_scalar("SELECT price_cents FROM products WHERE id = $1")
            .bind(item.product_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        total += price * item.quantity as i64;
    }
    let order = sqlx::query_as::<_, Order>("INSERT INTO orders (user_id, total_cents) VALUES (1, $1) RETURNING id, user_id, total_cents, status")
        .bind(total)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    tx.commit().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((StatusCode::CREATED, Json(order)))
}
"##;

const SHOP_LOGIN_TEST: &str = r##"use std::time::Duration;

use serde_json::json;
use shop_api::{build_app, AppState};
use sqlx::PgPool;

async fn base_url(pool: PgPool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_app(AppState::for_tests(pool));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(250)).await;
    format!("http://{addr}")
}

#[sqlx::test]
async fn login_returns_a_token(pool: PgPool) {
    let base = base_url(pool).await;
    let res = reqwest::Client::new().post(format!("{base}/auth/login")).json(&json!({"email": "amara@example.com", "password": "correct horse"})).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["token"].as_str().is_some());
}

#[sqlx::test]
async fn wrong_password_is_rejected(pool: PgPool) {
    let base = base_url(pool).await;
    let res = reqwest::Client::new().post(format!("{base}/auth/login")).json(&json!({"email": "amara@example.com", "password": "nope"})).send().await.unwrap();
    assert_eq!(res.status(), 401);
}
"##;

// fix-flaky-login-test
const TOKEN_FIX: &[(&str, &str)] = &[
    ("    if data.claims.exp < now {", "    if data.claims.exp <= now {"),
    ("        assert!(verify(&t, \"s3cret\", 1_000 + TTL_SECS).is_ok());", "        assert!(verify(&t, \"s3cret\", 1_000 + TTL_SECS - 1).is_ok());"),
    ("        assert!(matches!(verify(&t, \"s3cret\", 1_000 + TTL_SECS + 1), Err(AuthError::Expired)));", "        assert!(matches!(verify(&t, \"s3cret\", 1_000 + TTL_SECS), Err(AuthError::Expired)));"),
];

const LOGIN_TEST_FIX: &[(&str, &str)] = &[
    ("use std::time::Duration;\n\nuse serde_json::json;\nuse shop_api::{build_app, AppState};", "mod support;\n\nuse serde_json::json;\nuse shop_api::{build_app, AppState};"),
    ("    tokio::time::sleep(Duration::from_millis(250)).await;\n    format!(\"http://{addr}\")\n", "    let base = format!(\"http://{addr}\");\n    support::wait_until_ready(&base).await;\n    base\n"),
    (
        "    assert_eq!(res.status(), 401);\n}\n",
        "    assert_eq!(res.status(), 401);\n    let body: serde_json::Value = res.json().await.unwrap();\n    assert_eq!(body[\"error\"], \"invalid credentials\");\n}\n\n#[sqlx::test]\nasync fn an_unknown_email_is_rejected_the_same_way(pool: PgPool) {\n    let base = base_url(pool).await;\n    let res = reqwest::Client::new().post(format!(\"{base}/auth/login\")).json(&json!({\"email\": \"nobody@example.com\", \"password\": support::PASSWORD})).send().await.unwrap();\n    assert_eq!(res.status(), 401);\n    let body: serde_json::Value = res.json().await.unwrap();\n    assert_eq!(body[\"error\"], \"invalid credentials\");\n}\n",
    ),
];

const SUPPORT_MOD: &str = r##"//! Helpers the integration tests share.
use std::time::Duration;

pub const EMAIL: &str = "amara@example.com";
pub const PASSWORD: &str = "correct horse";

/// Waits until the server at `base` answers /health, instead of sleeping and hoping it is up.
pub async fn wait_until_ready(base: &str) {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(res) = client.get(format!("{base}/health")).send().await {
            if res.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the server at {base} was not ready after 2 s");
}
"##;

// reply-to-the-api-review
const ORDERS_PAGES: &[(&str, &str)] = &[
    ("    extract::{Path, State},", "    extract::{Path, Query, State},"),
    (
        "        .route(\"/orders\", get(list).post(create))\n        .route(\"/orders/:id\", get(get_one))\n}\n",
        "        .route(\"/orders\", get(list_orders).post(create_order))\n        .route(\"/orders/:id\", get(get_order))\n}\n\n/// `?page=2&per_page=25`: pages count from 1, and a page holds at most 100 orders.\n#[derive(Debug, Deserialize)]\npub struct Page {\n    #[serde(default = \"Page::first\")]\n    pub page: i64,\n    #[serde(default = \"Page::default_per_page\")]\n    pub per_page: i64,\n}\n\nimpl Page {\n    fn first() -> i64 {\n        1\n    }\n    fn default_per_page() -> i64 {\n        50\n    }\n    fn limit(&self) -> i64 {\n        self.per_page.clamp(1, 100)\n    }\n    fn offset(&self) -> i64 {\n        (self.page.max(1) - 1) * self.limit()\n    }\n}\n",
    ),
    (
        "async fn list(State(state): State<AppState>) -> Result<Json<Vec<Order>>, StatusCode> {\n    let orders = sqlx::query_as::<_, Order>(\"SELECT id, user_id, total_cents, status FROM orders ORDER BY id DESC LIMIT 50\")\n        .fetch_all(&state.db)\n",
        "async fn list_orders(State(state): State<AppState>, Query(page): Query<Page>) -> Result<Json<Vec<Order>>, StatusCode> {\n    let orders = sqlx::query_as::<_, Order>(\"SELECT id, user_id, total_cents, status FROM orders ORDER BY id DESC LIMIT $1 OFFSET $2\")\n        .bind(page.limit())\n        .bind(page.offset())\n        .fetch_all(&state.db)\n",
    ),
    ("async fn get_one(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Order>, StatusCode> {\n    sqlx::query_as::<_, Order>(\"SELECT id, user_id, total_cents, status FROM orders WHERE id = $1\")", "async fn get_order(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Order>, StatusCode> {\n    sqlx::query_as::<_, Order>(\"SELECT id, user_id, total_cents, status FROM orders WHERE id = $1\")"),
    ("async fn create(State(state)", "async fn create_order(State(state)"),
];

const API_DOC_PAGES: &[(&str, &str)] = &[(
    "Returns the caller's orders, newest first.\nAt most 50 are returned.\n",
    "Returns the caller's orders, newest first, one page at a time.\n\n| Query      | Default | Meaning                                 |\n| ---------- | ------- | --------------------------------------- |\n| `page`     | 1       | Which page, counting from 1.            |\n| `per_page` | 50      | Orders on a page, from 1 to 100.        |\n\nA page past the end is an empty list, not an error.\n\n```\nGET /orders?page=2&per_page=25\n```\n\nThe orders are the same ones in the same order whichever page size you pick.\n",
)];

const ORDERS_TEST: &str = r##"use sqlx::PgPool;

async fn spawn_app(pool: PgPool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = shop_api::build_app(shop_api::AppState::for_tests(pool));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[sqlx::test(fixtures("users", "orders"))]
async fn orders_come_in_pages(pool: PgPool) {
    let base = spawn_app(pool).await;
    let client = reqwest::Client::new();
    let first: Vec<serde_json::Value> = client.get(format!("{base}/orders?page=1&per_page=2")).send().await.unwrap().json().await.unwrap();
    let second: Vec<serde_json::Value> = client.get(format!("{base}/orders?page=2&per_page=2")).send().await.unwrap().json().await.unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    assert!(first[1]["id"].as_i64() > second[0]["id"].as_i64(), "newest first across pages");
}

#[sqlx::test(fixtures("users", "orders"))]
async fn a_page_past_the_end_is_empty(pool: PgPool) {
    let base = spawn_app(pool).await;
    let res = reqwest::get(format!("{base}/orders?page=99")).await.unwrap();
    assert_eq!(res.status(), 200);
    let body: Vec<serde_json::Value> = res.json().await.unwrap();
    assert!(body.is_empty());
}
"##;

// feature/refunds
const REFUND_SQL: &str = r##"CREATE TABLE refunds (
    id BIGSERIAL PRIMARY KEY,
    order_id BIGINT NOT NULL REFERENCES orders (id),
    amount_cents BIGINT NOT NULL CHECK (amount_cents > 0),
    reason TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX refunds_order_id_idx ON refunds (order_id);
"##;

const ORDERS_REFUND: &[(&str, &str)] = &[
    ("    routing::get,\n", "    routing::{get, post},\n"),
    ("        .route(\"/orders/:id\", get(get_one))\n", "        .route(\"/orders/:id\", get(get_one))\n        .route(\"/orders/:id/refund\", post(refund))\n"),
    (
        "async fn create(State(state)",
        "#[derive(Debug, Deserialize)]\npub struct Refund {\n    pub amount_cents: i64,\n    #[serde(default)]\n    pub reason: String,\n}\n\nasync fn refund(State(state): State<AppState>, Path(id): Path<i64>, Json(body): Json<Refund>) -> Result<StatusCode, StatusCode> {\n    let total: i64 = sqlx::query_scalar(\"SELECT total_cents FROM orders WHERE id = $1\")\n        .bind(id)\n        .fetch_optional(&state.db)\n        .await\n        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?\n        .ok_or(StatusCode::NOT_FOUND)?;\n    if body.amount_cents <= 0 || body.amount_cents > total {\n        return Err(StatusCode::UNPROCESSABLE_ENTITY);\n    }\n    sqlx::query(\"INSERT INTO refunds (order_id, amount_cents, reason) VALUES ($1, $2, $3)\")\n        .bind(id)\n        .bind(body.amount_cents)\n        .bind(&body.reason)\n        .execute(&state.db)\n        .await\n        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;\n    Ok(StatusCode::CREATED)\n}\n\nasync fn create(State(state)",
    ),
];

// amara/order-export-csv
const ORDERS_CSV: &[(&str, &str)] = &[
    ("        .route(\"/orders/:id\", get(get_one))\n", "        .route(\"/orders/:id\", get(get_one))\n        .route(\"/orders.csv\", get(export_csv))\n"),
    (
        "async fn create(State(state)",
        "async fn export_csv(State(state): State<AppState>) -> Result<String, StatusCode> {\n    let orders = sqlx::query_as::<_, Order>(\"SELECT id, user_id, total_cents, status FROM orders ORDER BY id\")\n        .fetch_all(&state.db)\n        .await\n        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;\n    let mut out = csv::Writer::from_writer(Vec::new());\n    for o in &orders {\n        out.serialize(o).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;\n    }\n    String::from_utf8(out.into_inner().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)\n}\n\nasync fn create(State(state)",
    ),
];

// any other agent in shop-api
const HEALTH_READY: &[(&str, &str)] = &[
    ("use axum::{routing::get, Router};\n", "use axum::{extract::State, http::StatusCode, routing::get, Json, Router};\nuse serde_json::json;\n"),
    (
        "    Router::new().route(\"/health\", get(health))\n}\n\nasync fn health() -> &'static str {\n    \"ok\"\n}\n",
        "    Router::new().route(\"/health\", get(health)).route(\"/ready\", get(ready))\n}\n\n/// Is the process up? Says nothing about what it depends on.\nasync fn health() -> &'static str {\n    \"ok\"\n}\n\n/// Can it serve? Asks Postgres and Redis, so a load balancer can hold traffic back while either is down.\n///\n/// Answers 200 with `{\"database\": true, \"cache\": true}`, or 503 with whichever is false.\nasync fn ready(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {\n    let db = sqlx::query(\"SELECT 1\").execute(&state.db).await.is_ok();\n    let mut cache = state.cache.clone();\n    let redis = redis::cmd(\"PING\").query_async::<String>(&mut cache).await.is_ok();\n    let code = if db && redis { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };\n    (code, Json(json!({ \"database\": db, \"cache\": redis })))\n}\n",
    ),
];

const HEALTH_TEST: &str = r##"use sqlx::PgPool;

async fn spawn_app(pool: PgPool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = shop_api::build_app(shop_api::AppState::for_tests(pool));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[sqlx::test]
async fn health_says_ok(pool: PgPool) {
    let base = spawn_app(pool).await;
    let res = reqwest::get(format!("{base}/health")).await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "ok");
}

#[sqlx::test]
async fn ready_checks_the_database_and_the_cache(pool: PgPool) {
    let base = spawn_app(pool).await;
    let res = reqwest::get(format!("{base}/ready")).await.unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["database"], true);
    assert_eq!(body["cache"], true);
}

#[sqlx::test]
async fn ready_turns_503_when_the_database_is_gone(pool: PgPool) {
    let base = spawn_app(pool.clone()).await;
    pool.close().await;
    let res = reqwest::get(format!("{base}/ready")).await.unwrap();
    assert_eq!(res.status(), 503);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["database"], false);
}
"##;

// ------------------------------------------------------------------ sphere

const SPHERE: Rp = Rp {
    name: "sphere",
    remote: "git@github.com:you/sphere.git",
    slug: "you/sphere",
    people: [("You", "you@example.com"), ("Ravi Menon", "ravi@example.com")],
    files: SPHERE_FILES,
    history: SPHERE_HISTORY,
    origin: "s14",
    dirty: &[Ch { path: "crates/sphered/src/workspace.rs", edits: WORKSPACE_DIRTY, new: None }],
    lines: SPHERE_LINES,
    generic: Line {
        branch: "",
        fork: "s14",
        commits: &[("sphered: cancel a task and reap the idle ones", 0.2), ("Test the task table", 0.1)],
        changes: &[
            Ch { path: "crates/sphered/src/manager.rs", edits: MANAGER_CANCEL, new: None },
            Ch { path: "crates/sphered/tests/tasks.rs", edits: &[], new: Some(TASKS_TEST) },
        ],
        dirty: &[],
        worktree: true,
        local: true,
        remote: false,
        in_main: false,
        pr: None,
    },
};

const SPHERE_HISTORY: &[Cm] = &[
    Cm { key: "s01", parents: &[], age: 336.0, who: 0, subject: "Initial commit" },
    Cm { key: "s02", parents: &["s01"], age: 330.0, who: 0, subject: "Workspace layout: app, daemon, proto" },
    Cm { key: "s03", parents: &["s02"], age: 300.0, who: 0, subject: "sphered: task store and worktrees" },
    Cm { key: "s04", parents: &["s03"], age: 280.0, who: 1, subject: "sphere-app: the orbit view" },
    Cm { key: "s05", parents: &["s04"], age: 250.0, who: 0, subject: "Containers app" },
    Cm { key: "s06", parents: &["s05"], age: 226.0, who: 1, subject: "Ports & logs app" },
    Cm { key: "s07", parents: &["s06"], age: 200.0, who: 0, subject: "System app" },
    Cm { key: "s08", parents: &["s07"], age: 170.0, who: 0, subject: "Git app: graph and staging" },
    Cm { key: "s09", parents: &["s08"], age: 140.0, who: 1, subject: "Code app: review an agent's diff" },
    Cm { key: "s10", parents: &["s09"], age: 110.0, who: 0, subject: "Keep agents' worktrees across restarts" },
    Cm { key: "s11", parents: &["s10"], age: 84.0, who: 1, subject: "Fix slug collisions when two tasks share a title" },
    Cm { key: "s12", parents: &["s11"], age: 60.0, who: 0, subject: "docs: architecture overview" },
    Cm { key: "s13", parents: &["s12"], age: 36.0, who: 1, subject: "sphered: stream agent output over the socket" },
    Cm { key: "s14", parents: &["s13"], age: 20.0, who: 0, subject: "Release 0.8.1" },
];

const SPHERE_LINES: &[Line] = &[
    Line {
        branch: "sphere/draft-release-notes",
        fork: "s14",
        commits: &[("Draft the 0.9.0 release notes", 4.0)],
        changes: &[Ch { path: "RELEASE_NOTES.md", edits: &[], new: Some(RELEASE_NOTES) }],
        dirty: &[],
        worktree: true,
        local: true,
        remote: true,
        in_main: false,
        pr: Some(Pr { number: 214, title: "Draft the 0.9.0 release notes", author: "you", draft: false, review: "REVIEW_REQUIRED", checks: &[("fmt", "SUCCESS"), ("clippy", "SUCCESS"), ("test", "SUCCESS")], updated: 3.5 }),
    },
    Line {
        branch: "docs-refresh",
        fork: "s12",
        commits: &[("docs: describe the socket protocol", 30.0)],
        changes: &[Ch { path: "docs/architecture.md", edits: &[("## Layout\n", "## The socket\n\nThe app and sphered talk over a Unix socket in `~/.sphere/run`. Each message is one line of JSON,\nand the daemon answers in the same order it was asked.\n\n## Layout\n")], new: None }],
        dirty: &[],
        worktree: false,
        local: true,
        remote: false,
        in_main: false,
        pr: None,
    },
    Line {
        branch: "feature/stream-output",
        fork: "s13",
        commits: &[("Add an Output message to the protocol", 30.0), ("sphere-app: show streamed output in the Code app", 12.0)],
        changes: &[Ch { path: "crates/sphere-proto/src/lib.rs", edits: PROTO_OUTPUT, new: None }, Ch { path: "crates/sphere-app/src/main.rs", edits: &[("mod ports;\n", "mod output;\nmod ports;\n")], new: None }],
        dirty: &[],
        worktree: false,
        local: false,
        remote: true,
        in_main: false,
        pr: Some(Pr { number: 209, title: "Stream agent output into the Code app", author: "ravi-menon", draft: false, review: "APPROVED", checks: &[("fmt", "SUCCESS"), ("clippy", "SUCCESS"), ("test", "PENDING")], updated: 11.0 }),
    },
];

const SPHERE_FILES: &[(&str, &str)] = &[
    (".gitignore", "/target\n*.log\n.DS_Store\n"),
    ("Cargo.toml", SPHERE_CARGO),
    ("README.md", SPHERE_README),
    ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.82.0\"\ncomponents = [\"clippy\", \"rustfmt\"]\n"),
    ("docs/architecture.md", SPHERE_ARCH),
    ("scripts/install.sh", SPHERE_INSTALL),
    ("crates/sphere-app/Cargo.toml", SPHERE_APP_CARGO),
    ("crates/sphere-app/src/main.rs", SPHERE_APP_MAIN),
    ("crates/sphere-app/src/ports.rs", SPHERE_PORTS),
    ("crates/sphered/Cargo.toml", SPHERED_CARGO),
    ("crates/sphered/src/main.rs", SPHERED_MAIN),
    ("crates/sphered/src/manager.rs", SPHERED_MANAGER),
    ("crates/sphered/src/workspace.rs", SPHERED_WORKSPACE),
    ("crates/sphere-proto/Cargo.toml", "[package]\nname = \"sphere-proto\"\nversion.workspace = true\nedition.workspace = true\n\n[dependencies]\nserde.workspace = true\nserde_json.workspace = true\n"),
    ("crates/sphere-proto/src/lib.rs", SPHERE_PROTO),
    ("crates/sphere-proto/src/paths.rs", SPHERE_PATHS),
];

const SPHERE_CARGO: &str = r##"[workspace]
resolver = "2"
members = ["crates/sphere-app", "crates/sphered", "crates/sphere-proto"]

[workspace.package]
version = "0.8.1"
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "sync", "time", "process"] }
tracing = "0.1"
bevy = { version = "0.16", default-features = false }

[profile.release]
lto = "thin"
codegen-units = 1
"##;

const SPHERE_README: &str = r##"# Sphere

A desktop app for working with coding agents. Each agent gets its own git worktree, and you review what it
changed, run its tests and merge it, all from one window. Written in Rust on Bevy.

## Install

    ./scripts/install.sh

## Apps

- Code: review an agent's diff, run its tests, merge or discard it
- Git: your repos, the commit graph, staging and pull requests
- Containers, Ports & logs, System: what is running on this machine

## Status

Sphere is in daily use on macOS.
"##;

const SPHERE_ARCH: &str = r##"# Architecture

Two programs and a protocol.

## Layout

- `sphere-app` is the window. It draws the apps and never blocks a frame: anything slow runs on a background thread.
- `sphered` is the daemon. It keeps the task table and creates a git worktree for each agent.
- `sphere-proto` holds the messages they exchange and the paths they agree on.

## Worktrees

An agent works in `~/.sphere/worktrees/<repo>/<slug>` on a branch named `sphere/<slug>`. Discarding a task removes
the worktree and leaves the branch.
"##;

const SPHERE_INSTALL: &str = r##"#!/bin/sh
set -eu

cargo build --release -p sphere-app -p sphered
install -d "$HOME/.sphere/bin"
install target/release/sphered "$HOME/.sphere/bin/sphered"
echo "installed sphered to $HOME/.sphere/bin"
"##;

const SPHERE_APP_CARGO: &str = r##"[package]
name = "sphere-app"
version.workspace = true
edition.workspace = true

[dependencies]
bevy.workspace = true
serde.workspace = true
serde_json.workspace = true
sphere-proto = { path = "../sphere-proto" }
"##;

const SPHERE_APP_MAIN: &str = r##"//! The Sphere window.
mod containers;
mod gitapp;
mod ports;

use bevy::prelude::*;

fn main() {
    App::new().add_plugins(DefaultPlugins).run();
}
"##;

const SPHERE_PORTS: &str = r##"//! The Ports & logs app: every TCP port listening on this machine.
use crate::opsdata::Listener;

/// "all interfaces", "this machine" or the address itself.
pub fn bound(addr: &str) -> String {
    match addr {
        "*" => "all interfaces".into(),
        "127.0.0.1" | "[::1]" => format!("{addr} (this machine)"),
        other => other.to_string(),
    }
}

pub fn label(l: &Listener) -> String {
    match &l.container {
        Some(c) => format!("{} · {}", c, l.service),
        None => l.command.clone(),
    }
}
"##;

const SPHERED_CARGO: &str = r##"[package]
name = "sphered"
version.workspace = true
edition.workspace = true

[dependencies]
sphere-proto = { path = "../sphere-proto" }
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
"##;

const SPHERED_MAIN: &str = r##"mod manager;
mod workspace;

use std::time::Duration;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let socket = sphere_proto::paths::default_socket();
    let mut tasks = manager::Manager::new(Duration::from_secs(30 * 60));
    tracing::info!("sphered listening on {}", socket.display());
    let _ = &mut tasks;
    tokio::signal::ctrl_c().await
}
"##;

const SPHERED_MANAGER: &str = r##"//! The task table: every agent sphered runs, and what each one is doing.
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use sphere_proto::{TaskState, TaskView};

use crate::workspace;

pub struct Manager {
    tasks: HashMap<String, Task>,
    idle_timeout: Duration,
}

struct Task {
    view: TaskView,
    started: SystemTime,
}

impl Manager {
    pub fn new(idle_timeout: Duration) -> Manager {
        Manager { tasks: HashMap::new(), idle_timeout }
    }

    /// Starts a task: a worktree on `sphere/<slug>`, forked from `base`.
    pub fn start(&mut self, repo: &Path, title: &str, base: &str) -> Result<TaskView, String> {
        let slug = workspace::slugify(title);
        let workdir = workspace::create(repo, &slug, base)?;
        let view = TaskView { id: slug.clone(), title: title.to_string(), workdir: workdir.display().to_string(), branch: format!("sphere/{slug}"), base: base.to_string(), state: TaskState::Working };
        self.tasks.insert(slug, Task { view: view.clone(), started: SystemTime::now() });
        Ok(view)
    }

    pub fn finish(&mut self, id: &str) -> Option<TaskView> {
        let t = self.tasks.get_mut(id)?;
        t.view.state = TaskState::Done;
        Some(t.view.clone())
    }

    pub fn list(&self) -> Vec<TaskView> {
        self.tasks.values().map(|t| t.view.clone()).collect()
    }
}
"##;

const MANAGER_CANCEL: &[(&str, &str)] = &[
    ("use std::time::{Duration, SystemTime};\n", "use std::time::{Duration, SystemTime};\n\nuse tracing::info;\n"),
    (
        "    pub fn list(&self) -> Vec<TaskView> {\n        self.tasks.values().map(|t| t.view.clone()).collect()\n    }\n",
        "    /// Stops a task that is still working. Its worktree and branch stay, so nothing it did is lost.\n    pub fn cancel(&mut self, id: &str) -> Result<TaskView, String> {\n        let t = self.tasks.get_mut(id).ok_or_else(|| format!(\"no task {id}\"))?;\n        if t.view.state != TaskState::Working {\n            return Err(format!(\"{id} is not working\"));\n        }\n        t.view.state = TaskState::Cancelled;\n        info!(\"cancelled {id}\");\n        Ok(t.view.clone())\n    }\n\n    /// Marks tasks that have been working longer than the idle timeout as stuck, and says which.\n    pub fn reap_idle(&mut self, now: SystemTime) -> Vec<String> {\n        let mut reaped = Vec::new();\n        for (id, t) in self.tasks.iter_mut() {\n            let age = now.duration_since(t.started).unwrap_or_default();\n            if t.view.state == TaskState::Working && age > self.idle_timeout {\n                t.view.state = TaskState::Stuck;\n                reaped.push(id.clone());\n            }\n        }\n        reaped.sort();\n        reaped\n    }\n\n    /// Tasks in the order they started.\n    pub fn list(&self) -> Vec<TaskView> {\n        let mut all: Vec<&Task> = self.tasks.values().collect();\n        all.sort_by_key(|t| t.started);\n        all.into_iter().map(|t| t.view.clone()).collect()\n    }\n",
    ),
];

const TASKS_TEST: &str = r##"use std::path::Path;
use std::time::{Duration, SystemTime};

use sphere_proto::TaskState;
use sphered::manager::Manager;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn a_started_task_is_listed_working() {
    let mut m = Manager::new(Duration::from_secs(60));
    let t = m.start(repo(), "Fix the flaky test", "main").unwrap();
    assert_eq!(t.branch, "sphere/fix-the-flaky-test");
    assert_eq!(m.list()[0].state, TaskState::Working);
}

#[test]
fn cancel_stops_a_working_task_once() {
    let mut m = Manager::new(Duration::from_secs(60));
    let t = m.start(repo(), "Try something", "main").unwrap();
    assert_eq!(m.cancel(&t.id).unwrap().state, TaskState::Cancelled);
    assert!(m.cancel(&t.id).unwrap_err().contains("not working"));
    assert!(m.cancel("nope").unwrap_err().contains("no task"));
}

#[test]
fn reap_idle_only_takes_tasks_past_the_timeout() {
    let mut m = Manager::new(Duration::from_secs(60));
    let t = m.start(repo(), "Slow one", "main").unwrap();
    assert!(m.reap_idle(SystemTime::now()).is_empty());
    let later = SystemTime::now() + Duration::from_secs(120);
    assert_eq!(m.reap_idle(later), vec![t.id.clone()]);
    assert!(m.reap_idle(later).is_empty());
}
"##;

const SPHERED_WORKSPACE: &str = r##"//! Where an agent works: a git worktree of the task's repo under ~/.sphere/worktrees.
use std::path::{Path, PathBuf};
use std::process::Command;

use sphere_proto::paths::default_state_dir;

/// `~/.sphere/worktrees/<repo>/<slug>`
pub fn worktree_path(repo: &Path, slug: &str) -> PathBuf {
    let name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
    default_state_dir().join("worktrees").join(name).join(slug)
}

/// A branch-safe slug from a task title: "Fix the flaky test!" becomes "fix-the-flaky-test".
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

/// Creates the worktree on a new branch `sphere/<slug>` from `base`.
pub fn create(repo: &Path, slug: &str, base: &str) -> Result<PathBuf, String> {
    let path = worktree_path(repo, slug);
    let out = Command::new("git").arg("-C").arg(repo).args(["worktree", "add", "-b", &format!("sphere/{slug}")]).arg(&path).arg(base).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(path)
}
"##;

const WORKSPACE_DIRTY: &[(&str, &str)] = &[
    ("    out.trim_matches('-').chars().take(40).collect()\n}", "    // cutting at 40 can leave a dash at the end\n    let cut: String = out.trim_matches('-').chars().take(40).collect();\n    cut.trim_end_matches('-').to_string()\n}"),
    ("    let path = worktree_path(repo, slug);\n    let out = Command::new(", "    let path = worktree_path(repo, slug);\n    if path.exists() {\n        return Err(format!(\"{} is already there\", path.display()));\n    }\n    let out = Command::new("),
];

const SPHERE_PROTO: &str = r##"//! The messages sphere-app and sphered exchange, one JSON object per line.
use serde::{Deserialize, Serialize};

pub mod paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Queued,
    Working,
    Waiting,
    Done,
    Cancelled,
    Stuck,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub workdir: String,
    pub branch: String,
    pub base: String,
    pub state: TaskState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    List,
    Start { repo: String, title: String, base: String },
    Finish { id: String },
}
"##;

const PROTO_OUTPUT: &[(&str, &str)] = &[(
    "    Finish { id: String },\n}\n",
    "    Finish { id: String },\n    /// Asks for an agent's output from line `from` on.\n    Output { id: String, from: usize },\n}\n\n/// What the daemon answers to `Request::Output`.\n#[derive(Clone, Debug, Serialize, Deserialize)]\npub struct OutputChunk {\n    pub id: String,\n    pub from: usize,\n    pub lines: Vec<String>,\n    pub done: bool,\n}\n",
)];

const SPHERE_PATHS: &str = r##"//! Where Sphere keeps things. Everything is overridable by config; these are only the defaults.
use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/.sphere`
pub fn default_state_dir() -> PathBuf {
    home().join(".sphere")
}

pub fn default_socket() -> PathBuf {
    default_state_dir().join("run").join("sphered.sock")
}
"##;

const RELEASE_NOTES: &str = r##"# Release notes

## 0.9.0 (unreleased)

### New

- Code app: run an agent's tests from the review screen and watch them pass or fail as they go.
- Git app: pull requests for the repo, with their checks. Press M twice to squash-merge one.
- Containers: pull a newer image for a container from the keyboard (P), then restart it.
- Ports & logs: the container behind a published port is named next to it.
- sphered keeps an agent's worktree when the app restarts in the middle of a task.

### Changed

- Agents' branches are now named `sphere/<slug>`, and the worktree is `~/.sphere/worktrees/<repo>/<slug>`.
- The System app samples once a second instead of twice, and uses less CPU doing it.
- Merging an agent's branch refuses when the repo has uncommitted changes, and says which branch to switch to.

### Fixed

- Two tasks with the same title no longer fight over one worktree.
- A port that listened on both IPv4 and IPv6 was listed twice.
- A log line with a tab in it no longer shifted the columns of the Containers app.
- The Git graph drew a merge commit one lane too far to the right.

## 0.8.1 (2026-10-09)

- Fixed the Ports app listing a port twice.
- Fixed a crash when a worktree was deleted while the Code app was showing it.
- sphered streams agent output over the socket instead of polling for it.

## 0.8.0 (2026-09-24)

### New

- The Git app: your repos at a glance, the commit graph across branches, staging and the commit box.
- Code app: review an agent's changes file by file, with the hunks it touched.
- System app: processor, memory, GPU, network, disk and battery, sampled once a second.

### Changed

- The orbit view puts the apps you used last nearest the front.
- Agents are listed oldest first, and a finished agent says Merged once its branch is part of main.

### Fixed

- The Containers app showed a stopped container's old health status.
- Opening the Ports app with Docker not running looked like an empty machine instead of saying why.
"##;

