# Fitnab 🚀

**Fitnab** is a high-performance, hardened, and low-maintenance Torznab indexer designed to seamlessly bridge game repack metadata into your favorite media management tools (like Prowlarr, Sonarr, or Questarr).

Written in **Rust**, Fitnab is built for speed, efficiency, and rock-solid stability.

---

## ✨ Key Features

### 🧠 Intelligent "Chain of Custody" Sync
Fitnab doesn't waste time or resources. On startup, it intelligently compares the RSS feed's history against its local database.
- **Fast Startup**: If the RSS feed is continuous, it skips the heavy A-Z library crawl.
- **Reactive Updates**: If a game is updated (e.g., a new version), Fitnab automatically resets its indexing status to pull fresh metadata on the next request.

### ⚡ Just-In-Time (JIT) Metadata
Unlike traditional indexers that scrape everything upfront, Fitnab uses a JIT strategy:
- **Passive Browsing**: Browsing "Latest" releases is near-instantaneous.
- **On-Demand Deep Scans**: Magnet links and file sizes are fetched only when you perform a targeted search or click download.

### 🛡️ Hardened & Lean
- **Distroless Base**: The production image is built on `gcr.io/distroless/static`, containing zero shells or utilities—making it extremely secure.
- **Tiny Footprint**: The entire container image is approximately **20MB**.
- **Static Binary**: Compiled against `musl` for zero external dependencies.

### 📊 Robust Health Checks
- **Parallel Scraping**: Queries multiple UDP trackers simultaneously.
- **Max Health Strategy**: Automatically selects the highest seeder count from all trackers to ensure accurate health data even if some trackers are out of sync.

---

## 🚀 Quick Start

### Deployment via Docker (Recommended)
The easiest way to run Fitnab is using the pre-built image from **GHCR**.

1. Create a `docker-compose.yml`:
   ```yaml
   services:
     fitnab:
       image: ghcr.io/roormonger/fitnab:latest
       container_name: fitnab
       restart: unless-stopped
       volumes:
         - ./data:/app/data
       ports:
         - "3000:3000"
       environment:
         - RUST_LOG=info
   ```

2. Launch the indexer:
   ```bash
   docker-compose up -d
   ```

### Adding to Prowlarr
- **Indexer Type**: Generic Torznab
- **URL**: `http://your-server-ip:3000`
- **API Key**: (None required)
- **Categories**: 4000, 4050 (PC/Games)

---

## 🛠️ Tech Stack
- **Language**: [Rust](https://www.rust-lang.org/)
- **Framework**: [Axum](https://github.com/tokio-rs/axum)
- **Database**: [SQLite](https://sqlite.org/) (via [SQLx](https://github.com/launchbadge/sqlx))
- **CI/CD**: GitHub Actions pushing to GHCR.io

---

## 📝 License
This project is for educational purposes only. Please support game developers by purchasing titles whenever possible.
