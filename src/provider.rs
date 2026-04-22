use async_trait::async_trait;
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct GameEntry {
    pub id: String,
    #[allow(dead_code)]
    pub provider: String,
    pub title: String,
    pub post_url: String,
    pub magnet_link: Option<String>,
    #[allow(dead_code)]
    pub torrent_blob: Option<Vec<u8>>,
    pub is_indexed: bool,
    pub size_bytes: Option<i64>,
    pub published_at: Option<String>,
    pub info_hash: Option<String>,
    pub seeders: Option<i32>,
    pub leechers: Option<i32>,
    pub completed: Option<i32>,
}

pub struct Metadata {
    pub magnet_link: Option<String>,
    pub size_bytes: Option<i64>,
    pub published_at: Option<String>,
    pub seeders: Option<i32>,
    pub leechers: Option<i32>,
    pub completed: Option<i32>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Returns the provider's unique identifier.
    fn name(&self) -> &'static str;

    /// Sync the latest A-Z list and store un-indexed entries in the DB.
    async fn sync_library(&self, pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Perform a deep fetch on a specific game to extract its magnet link, size, and date.
    async fn fetch_metadata(&self, post_url: &str) -> Result<Metadata, Box<dyn std::error::Error + Send + Sync>>;

    /// Sync the RSS feed to get the latest releases. Returns the timestamp of the oldest item in the feed.
    async fn sync_rss(&self, pool: &SqlitePool) -> Result<Option<chrono::DateTime<chrono::Utc>>, Box<dyn std::error::Error + Send + Sync>>;
}
