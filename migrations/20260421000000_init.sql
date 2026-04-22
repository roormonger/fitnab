-- Initial schema for Fitnab
CREATE TABLE IF NOT EXISTS games (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    title TEXT NOT NULL,
    post_url TEXT NOT NULL,
    magnet_link TEXT,
    torrent_blob BLOB,
    is_indexed BOOLEAN NOT NULL DEFAULT 0,
    size_bytes INTEGER,
    published_at TEXT,
    info_hash TEXT,
    seeders INTEGER,
    leechers INTEGER,
    completed INTEGER,
    last_updated DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS kv_store (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_games_title ON games(title);
CREATE INDEX IF NOT EXISTS idx_games_is_indexed ON games(is_indexed);
