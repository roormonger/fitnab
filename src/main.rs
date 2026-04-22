mod provider;
mod fitgirl;

use axum::{
    extract::{Query, Path, State, Host},
    response::{IntoResponse, Redirect},
    routing::get,
    Router,
    http::{StatusCode, header},
};
use provider::Provider;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::{net::SocketAddr, sync::Arc, collections::HashMap};

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    fitgirl_provider: Arc<fitgirl::FitgirlProvider>,
}

impl AppState {
    /// Performs a deep fetch for a game's metadata and updates the database.
    /// Returns the updated GameEntry.
    async fn jit_index_game(&self, mut game: provider::GameEntry) -> provider::GameEntry {
        if game.is_indexed {
            return game;
        }

        tracing::info!("JIT: Fetching metadata for {}", game.title);
        if let Ok(metadata) = self.fitgirl_provider.fetch_metadata(&game.post_url).await {
            let (hash, _) = metadata.magnet_link.as_ref()
                .map(|m| extract_info_hash_and_trackers(m))
                .unwrap_or((None, vec![]));
            
            // Update DB
            let _ = sqlx::query(
                "UPDATE games SET is_indexed = 1, magnet_link = ?1, size_bytes = ?2, published_at = ?3, info_hash = ?4, seeders = ?5, leechers = ?6, completed = ?7 WHERE id = ?8"
            )
            .bind(&metadata.magnet_link)
            .bind(metadata.size_bytes)
            .bind(&metadata.published_at)
            .bind(&hash)
            .bind(metadata.seeders)
            .bind(metadata.leechers)
            .bind(metadata.completed)
            .bind(&game.id)
            .execute(&self.pool).await;

            game.magnet_link = metadata.magnet_link;
            game.size_bytes = metadata.size_bytes;
            game.published_at = metadata.published_at;
            game.info_hash = hash;
            game.seeders = metadata.seeders;
            game.leechers = metadata.leechers;
            game.completed = metadata.completed;
            game.is_indexed = true;
        } else {
            tracing::error!("JIT: Failed to fetch metadata for {}", game.title);
        }
        game
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Fitnab...");

    let db_url = "sqlite:/app/data/fitnab.db?mode=rwc";
    
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

    tracing::info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;

    let provider = Arc::new(fitgirl::FitgirlProvider::new());
    
    let state = AppState {
        pool: pool.clone(),
        fitgirl_provider: provider.clone(),
    };

    // Spawn background sync
    let sync_pool = pool.clone();
    let sync_provider = provider.clone();
    tokio::spawn(async move {
        // Initial sync logic using "Chain of Custody"
        let last_sync: Option<(String,)> = sqlx::query_as("SELECT value FROM kv_store WHERE key = 'last_full_sync_at'")
            .fetch_optional(&sync_pool)
            .await
            .unwrap_or(None);

        let last_sync_dt = last_sync.and_then(|(ts_str,)| chrono::DateTime::parse_from_rfc3339(&ts_str).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        // Step 1: Run RSS sync first to see what's new and check continuity
        let rss_result = sync_provider.sync_rss(&sync_pool).await;
        let mut should_full_sync = true;

        match (last_sync_dt, rss_result) {
            (Some(last_dt), Ok(Some(oldest_rss_dt))) => {
                if oldest_rss_dt <= last_dt {
                    tracing::info!("RSS continuity confirmed (Oldest RSS: {} <= Last Sync: {}). Skipping full A-Z crawl.", 
                        oldest_rss_dt.to_rfc3339(), last_dt.to_rfc3339());
                    should_full_sync = false;
                } else {
                    tracing::warn!("RSS gap detected (Oldest RSS: {} > Last Sync: {}). Full crawl required.", 
                        oldest_rss_dt.to_rfc3339(), last_dt.to_rfc3339());
                }
            }
            (None, _) => {
                tracing::info!("No previous sync record found. Full crawl required.");
            }
            (_, Err(e)) => {
                tracing::error!("RSS sync failed, falling back to full crawl: {:?}", e);
            }
            (Some(_), Ok(None)) => {
                tracing::warn!("RSS feed was empty or date parsing failed. Falling back to full crawl for safety.");
            }
        }

        if should_full_sync {
            tracing::info!("Starting full A-Z library sync...");
            if let Err(e) = sync_provider.sync_library(&sync_pool).await {
                tracing::error!("Full sync failed: {:?}", e);
            } else {
                let now_str = chrono::Utc::now().to_rfc3339();
                let _ = sqlx::query("INSERT OR REPLACE INTO kv_store (key, value) VALUES ('last_full_sync_at', ?)")
                    .bind(now_str)
                    .execute(&sync_pool)
                    .await;
                tracing::info!("Full sync completed and timestamp updated.");
            }
        } else {
            // Even if we skip, we update the timestamp to "now" because the RSS feed just confirmed we are current
            let now_str = chrono::Utc::now().to_rfc3339();
            let _ = sqlx::query("INSERT OR REPLACE INTO kv_store (key, value) VALUES ('last_full_sync_at', ?)")
                .bind(now_str)
                .execute(&sync_pool)
                .await;
        }

        // Continuous RSS monitoring (every 1 hour)
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // The first tick is immediate, so we skip it to wait 1 hour
        loop {
            interval.tick().await;
            if let Err(e) = sync_provider.sync_rss(&sync_pool).await {
                tracing::error!("Background RSS sync failed: {:?}", e);
            }
        }
    });

    let app = Router::new()
        .route("/api", get(torznab_api_handler))
        .route("/download/:id", get(download_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

mod scrape;


async fn torznab_api_handler(
    State(state): State<AppState>,
    Host(host): Host,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let t = params.get("t").map(|s| s.as_str()).unwrap_or("");

    match t {
        "caps" => {
            let caps = r#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server version="1.0" title="Fitnab" />
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="no" />
    <movie-search available="no" />
  </searching>
  <categories>
    <category id="4000" name="PC">
      <subcat id="4050" name="PC/Games" />
    </category>
  </categories>
</caps>"#;
            ([(header::CONTENT_TYPE, "application/xml")], caps.to_string())
        }
        "search" | "" => {
            let q = params.get("q").map(|s| s.as_str()).unwrap_or("");
            
            // Query DB
            let query_str = format!("%{}%", q);
            let results = match sqlx::query_as::<_, provider::GameEntry>(
                "SELECT id, provider, title, post_url, magnet_link, torrent_blob, is_indexed, size_bytes, published_at, info_hash, seeders, leechers, completed FROM games WHERE title LIKE ?1 ORDER BY published_at DESC LIMIT 50"
            )
            .bind(&query_str)
            .fetch_all(&state.pool).await {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("DB error: {}", e);
                    return ([(header::CONTENT_TYPE, "application/xml")], "<error>DB Error</error>".to_string());
                }
            };

            let mut items = String::new();
            for mut game in results {
                // JIT Fetch: If not indexed, go fetch metadata
                game = state.jit_index_game(game).await;

                // Live Health Check
                let mut seeders = game.seeders.unwrap_or(0) as u32;
                let mut leechers = game.leechers.unwrap_or(0) as u32;

                let (hash_opt, trackers) = match game.magnet_link.as_deref() {
                    Some(m) => extract_info_hash_and_trackers(m),
                    None => (game.info_hash.clone(), vec!["tracker.opentrackr.org:1337".to_string()]),
                };

                if let Some(hash_hex) = hash_opt {
                    if let Ok(hash_bytes) = hex::decode(&hash_hex) {
                        if let Ok(h_bytes) = hash_bytes.try_into() {
                            // Try trackers in parallel (take first success)
                            let mut trackers_to_try = trackers;
                            if trackers_to_try.is_empty() {
                                trackers_to_try.push("tracker.opentrackr.org:1337".to_string());
                            }
                            // Limit to top 3 trackers to avoid spamming
                            trackers_to_try.truncate(3);

                            tracing::debug!("Scraping health for {} using {} trackers", game.title, trackers_to_try.len());
                            
                            for tr_addr in trackers_to_try {
                                match scrape::scrape_tracker(&h_bytes, &tr_addr).await {
                                    Ok(res) => {
                                        seeders = res.seeders;
                                        leechers = res.leechers;
                                        tracing::debug!("Health for {}: {} seeders, {} peers (from {})", game.title, seeders, leechers, tr_addr);
                                        break; // Found one!
                                    }
                                    Err(e) => {
                                        tracing::debug!("Scrape failed for {} on {}: {}", game.title, tr_addr, e);
                                    }
                                }
                            }
                        }
                    }
                }

                let size = game.size_bytes.unwrap_or(1073741824); // Default 1GB if unknown
                let pub_date_str = game.published_at.as_deref().unwrap_or("2024-01-01T00:00:00Z");
                
                // Format date for RSS (RFC 822)
                let rfc_date = match chrono::DateTime::parse_from_rfc3339(pub_date_str) {
                    Ok(dt) => dt.to_rfc2822(),
                    Err(_) => "Mon, 01 Jan 2024 00:00:00 +0000".to_string(),
                };

                let enclosure_url = format!("http://{}/download/{}", host, game.id);

                items.push_str(&format!(
                    r#"
        <item>
            <title>{}</title>
            <guid>{}</guid>
            <link>{}</link>
            <pubDate>{}</pubDate>
            <size>{}</size>
            <category>4000</category>
            <category>4050</category>
            <torznab:attr name="category" value="4000" />
            <torznab:attr name="category" value="4050" />
            <torznab:attr name="seeders" value="{}" />
            <torznab:attr name="peers" value="{}" />
            <enclosure url="{}" length="{}" type="application/x-bittorrent" />
        </item>"#,
                    quick_xml::escape::escape(&game.title),
                    game.id,
                    quick_xml::escape::escape(&game.post_url),
                    rfc_date,
                    size,
                    seeders,
                    leechers,
                    quick_xml::escape::escape(&enclosure_url),
                    size
                ));
            }

            let rss = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
    <channel>
        <title>Fitnab</title>
        <description>FitGirl Torznab Indexer</description>
        <link>http://{}/</link>{}
    </channel>
</rss>"#,
                host, items
            );

            ([(header::CONTENT_TYPE, "application/rss+xml")], rss)
        }
        _ => ([(header::CONTENT_TYPE, "application/xml")], "<error>Unknown type</error>".to_string()),
    }
}

fn extract_info_hash_and_trackers(magnet: &str) -> (Option<String>, Vec<String>) {
    let hash_re = regex::Regex::new(r"btih:([a-zA-Z0-9]{32,40})").unwrap();
    let hash = hash_re.captures(magnet).map(|c| c.get(1).unwrap().as_str().to_lowercase());
    
    let tracker_re = regex::Regex::new(r"tr=([^&]+)").unwrap();
    let mut trackers = Vec::new();
    for caps in tracker_re.captures_iter(magnet) {
        if let Some(m) = caps.get(1) {
            let tr = urlencoding::decode(m.as_str()).unwrap_or_default().to_string();
            // We only support UDP trackers for scraping currently
            if tr.starts_with("udp://") {
                trackers.push(tr.replace("udp://", ""));
            }
        }
    }

    (hash, trackers)
}

async fn download_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let row_opt = sqlx::query_as::<_, provider::GameEntry>(
        "SELECT id, provider, title, post_url, magnet_link, torrent_blob, is_indexed, size_bytes, published_at, info_hash, seeders, leechers, completed FROM games WHERE id = ?1"
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let game = match row_opt {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "Game not found").into_response(),
    };

    let game = state.jit_index_game(game).await;

    if game.magnet_link.is_some() {
        return Redirect::temporary(&game.magnet_link.unwrap()).into_response();
    } else {
        return (StatusCode::NOT_FOUND, "Magnet link not found").into_response();
    }
}
