# Fitnab 🚀

**Fitnab** is a tiny 15 mb container thatacts as a torrent indexer for the fitgirl repack site serving torrent files only.

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

