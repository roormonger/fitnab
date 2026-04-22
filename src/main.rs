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
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    fitgirl_provider: Arc<fitgirl::FitgirlProvider>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Fitnab...");

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:fitnab.db".to_string());
    
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
            let results = match sqlx::query_as!(
                provider::GameEntry,
                "SELECT id as 'id!', provider as 'provider!', title as 'title!', post_url as 'post_url!', magnet_link, torrent_blob, is_indexed as 'is_indexed!: bool', size_bytes, published_at, info_hash FROM games WHERE title LIKE ?1 ORDER BY published_at DESC LIMIT 50",
                query_str
            ).fetch_all(&state.pool).await {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("DB error: {}", e);
                    return ([(header::CONTENT_TYPE, "application/xml")], "<error>DB Error</error>".to_string());
                }
            };

            let mut items = String::new();
            for mut game in results {
                // JIT Fetch: If not indexed, go fetch metadata
                if !game.is_indexed {
                    tracing::info!("JIT: Fetching metadata for {}", game.title);
                    if let Ok((magnet, size, date)) = state.fitgirl_provider.fetch_metadata(&game.post_url).await {
                        let hash = magnet.as_ref().and_then(|m| extract_info_hash(m));
                        
                        // Update DB
                        let _ = sqlx::query!(
                            "UPDATE games SET is_indexed = 1, magnet_link = ?1, size_bytes = ?2, published_at = ?3, info_hash = ?4 WHERE id = ?5",
                            magnet, size, date, hash, game.id
                        ).execute(&state.pool).await;

                        game.magnet_link = magnet;
                        game.size_bytes = size;
                        game.published_at = date;
                        game.info_hash = hash;
                        game.is_indexed = true;
                    }
                }

                // Live Health Check
                let mut seeders = 0;
                let mut leechers = 0;
                if let Some(ref hash_hex) = game.info_hash {
                    if let Ok(hash_bytes) = hex::decode(hash_hex) {
                        if let Ok(h_bytes) = hash_bytes.try_into() {
                            // Try a popular tracker
                            tracing::debug!("Scraping health for {}", game.title);
                            match scrape::scrape_tracker(&h_bytes, "tracker.opentrackr.org:1337").await {
                                Ok(res) => {
                                    seeders = res.seeders;
                                    leechers = res.leechers;
                                    tracing::debug!("Health for {}: {} seeders, {} peers", game.title, seeders, leechers);
                                }
                                Err(e) => {
                                    tracing::debug!("Scrape failed for {}: {}", game.title, e);
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

fn extract_info_hash(magnet: &str) -> Option<String> {
    // magnet:?xt=urn:btih:<HASH>&...
    // Support both 40-char hex and 32-char base32
    let re = regex::Regex::new(r"btih:([a-zA-Z0-9]{32,40})").unwrap();
    re.captures(magnet).map(|c| {
        let hash = c.get(1).unwrap().as_str().to_lowercase();
        if hash.len() == 40 {
            hash
        } else if hash.len() == 32 {
            // This is base32, we should ideally convert it to hex, 
            // but for now let's just return it and we'll handle it if needed.
            // FitGirl usually uses 40-char hex.
            hash
        } else {
            hash
        }
    })
}

async fn download_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let row_opt = sqlx::query_as!(
        provider::GameEntry,
        "SELECT id as 'id!', provider as 'provider!', title as 'title!', post_url as 'post_url!', magnet_link, torrent_blob, is_indexed as 'is_indexed!: bool', size_bytes, published_at, info_hash FROM games WHERE id = ?1", 
        id
    )
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let game = match row_opt {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "Game not found").into_response(),
    };

    if game.is_indexed && game.magnet_link.is_some() {
        return Redirect::temporary(&game.magnet_link.unwrap()).into_response();
    }

    // Do deep fetch
    match state.fitgirl_provider.fetch_metadata(&game.post_url).await {
        Ok((magnet_link, size, date)) => {
            if let Some(magnet) = magnet_link {
                let hash = extract_info_hash(&magnet);
                // Update DB
                let _ = sqlx::query!(
                    "UPDATE games SET is_indexed = 1, magnet_link = ?1, size_bytes = ?2, published_at = ?3, info_hash = ?4 WHERE id = ?5",
                    magnet, size, date, hash, id
                ).execute(&state.pool).await;
                
                return Redirect::temporary(&magnet).into_response();
            } else {
                return (StatusCode::NOT_FOUND, "Magnet link not found on FitGirl page").into_response();
            }
        }
        Err(e) => {
            tracing::error!("Fetch metadata failed: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to scrape FitGirl").into_response();
        }
    }
}
