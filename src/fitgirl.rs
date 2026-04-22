use crate::provider::Provider;
use async_trait::async_trait;
use scraper::{Html, Selector};
use sqlx::SqlitePool;
use std::time::Duration;
use rand::Rng;

pub struct FitgirlProvider {
    client: rquest::Client,
}

impl FitgirlProvider {
    pub fn new() -> Self {
        let client = rquest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build rquest client");

        Self { client }
    }
}

#[async_trait]
impl Provider for FitgirlProvider {
    fn name(&self) -> &'static str {
        "FitGirl Repacks"
    }

    async fn sync_library(&self, pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Syncing FitGirl A-Z library...");

        let mut page = 1;
        let mut total_inserted = 0;

        loop {
            let url = format!("https://fitgirl-repacks.site/all-my-repacks-a-z/?lcp_page0={}", page);
            tracing::debug!("Fetching page: {}", url);
            
            let res = self.client.get(&url).send().await?;
            if !res.status().is_success() {
                tracing::error!("Failed to fetch page {}: HTTP {}", page, res.status());
                break;
            }

            let html_content = res.text().await?;
            
            let mut items_to_insert = Vec::new();
            let (has_next, parsed_any) = {
                let document = Html::parse_document(&html_content);
                let ul_selector = Selector::parse("ul.lcp_catlist li a").unwrap();
                let next_selector = Selector::parse("ul.lcp_paginator li a[title='Next']").unwrap();

                let has_next = document.select(&next_selector).next().is_some();
                let parsed_any = document.select(&ul_selector).next().is_some();

                for element in document.select(&ul_selector) {
                    let title = element.text().collect::<Vec<_>>().join("");
                    if let Some(post_url) = element.value().attr("href") {
                        let id = post_url
                            .trim_end_matches('/')
                            .split('/')
                            .last()
                            .unwrap_or("unknown")
                            .to_string();
                        items_to_insert.push((id, title, post_url.to_string()));
                    }
                }
                (has_next, parsed_any)
            };

            let mut found_on_page = 0;
            let provider_name = self.name();
            
            let mut tx = pool.begin().await?;
            
            for (id, title, post_url) in items_to_insert {
                let inserted = sqlx::query!(
                    r#"
                    INSERT INTO games (id, provider, title, post_url, is_indexed)
                    VALUES (?1, ?2, ?3, ?4, 0)
                    ON CONFLICT(id) DO NOTHING
                    "#,
                    id,
                    provider_name,
                    title,
                    post_url
                )
                .execute(&mut *tx)
                .await?;

                if inserted.rows_affected() > 0 {
                    found_on_page += 1;
                    total_inserted += 1;
                }
            }
            
            tx.commit().await?;

            tracing::info!("Page {} parsed. Inserted {} new games.", page, found_on_page);

            if !has_next && !parsed_any {
                break;
            }

            page += 1;
            
            let delay_ms = rand::thread_rng().gen_range(1000..=3000);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        tracing::info!("Sync complete. Total new games inserted: {}", total_inserted);
        Ok(())
    }

    async fn fetch_metadata(&self, post_url: &str) -> Result<crate::provider::Metadata, Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Deep fetching metadata for: {}", post_url);
        
        let res = self.client.get(post_url).send().await?;
        let html_content = res.text().await?;
        let document = Html::parse_document(&html_content);
        
        let mut magnet_link = None;
        let mut size_bytes = None;
        let mut published_at = None;
        let seeders = None;
        let leechers = None;
        let completed = None;

        let a_selector = Selector::parse("a[href^='magnet:?']").unwrap();
        if let Some(magnet_elem) = document.select(&a_selector).next() {
            magnet_link = magnet_elem.value().attr("href").map(|s| s.to_string());
        }

        let time_selector = Selector::parse("time.entry-date").unwrap();
        if let Some(time_elem) = document.select(&time_selector).next() {
            published_at = time_elem.value().attr("datetime").map(|s| s.to_string());
        }

        let content_selector = Selector::parse(".entry-content").unwrap();
        if let Some(content) = document.select(&content_selector).next() {
            let full_text = content.text().collect::<Vec<_>>().join(" ");
            
            // 1. Try to get "Repack Size" explicitly (Best source for size)
            let repack_re = regex::Regex::new(r"(?i)Repack\s+Size\s*[:\s]+\s*(?:from\s+)?([\d./]+)\s*([KMGT]B)").unwrap();
            if let Some(caps) = repack_re.captures(&full_text) {
                let raw_val = caps.get(1).unwrap().as_str();
                // If it's a range like "55/55.1", take the last number
                let val_str = raw_val.split('/').last().unwrap_or(raw_val);
                let val: f64 = val_str.parse().unwrap_or(0.0);
                
                let unit = caps.get(2).unwrap().as_str().to_uppercase();
                let multiplier = match unit.as_str() {
                    "KB" => 1024,
                    "MB" => 1024 * 1024,
                    "GB" => 1024 * 1024 * 1024,
                    "TB" => 1024 * 1024 * 1024 * 1024,
                    _ => 1,
                };
                size_bytes = Some((val * multiplier as f64) as i64);
                tracing::debug!("Found repack size: {} bytes", size_bytes.unwrap());
            }

            // 2. Last resort: General size regex (strictly avoiding "Original Size")
            if size_bytes.is_none() {
                // Match anything like "Size: 10 GB" but we'll check the prefix in Rust
                let general_re = regex::Regex::new(r"(?i)(Original\s+)?Size\s*[:\s]+\s*(?:from\s+)?([\d./]+)\s*([KMGT]B)").unwrap();
                for caps in general_re.captures_iter(&full_text) {
                    // Skip if it matched "Original Size"
                    if caps.get(1).is_some() {
                        continue;
                    }

                    let raw_val = caps.get(2).unwrap().as_str();
                    let val_str = raw_val.split('/').last().unwrap_or(raw_val);
                    let val: f64 = val_str.parse().unwrap_or(0.0);

                    let unit = caps.get(3).unwrap().as_str().to_uppercase();
                    let multiplier = match unit.as_str() {
                        "KB" => 1024,
                        "MB" => 1024 * 1024,
                        "GB" => 1024 * 1024 * 1024,
                        "TB" => 1024 * 1024 * 1024 * 1024,
                        _ => 1,
                    };
                    size_bytes = Some((val * multiplier as f64) as i64);
                    tracing::debug!("Found general size: {} bytes", size_bytes.unwrap());
                    break;
                }
            }
        }

        Ok(crate::provider::Metadata {
            magnet_link,
            size_bytes,
            published_at,
            seeders,
            leechers,
            completed,
        })
    }

    async fn sync_rss(&self, pool: &SqlitePool) -> Result<Option<chrono::DateTime<chrono::Utc>>, Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Syncing FitGirl RSS feed...");
        
        let url = "https://fitgirl-repacks.site/feed/";
        let res = self.client.get(url).send().await?;
        if !res.status().is_success() {
            tracing::error!("Failed to fetch RSS feed: HTTP {}", res.status());
            return Ok(None);
        }

        let xml_content = res.text().await?;
        
        let mut reader = quick_xml::Reader::from_str(&xml_content);
        reader.config_mut().trim_text(true);

        let mut in_item = false;
        let mut current_title = String::new();
        let mut current_link = String::new();
        let mut current_pub_date = String::new();
        let mut current_tag = String::new();
        
        let mut items_to_insert = Vec::new();

        loop {
            use quick_xml::events::Event;
            match reader.read_event() {
                Ok(Event::Start(ref e)) => {
                    let name_str = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if name_str == "item" {
                        in_item = true;
                        current_title.clear();
                        current_link.clear();
                        current_pub_date.clear();
                    }
                    if in_item {
                        current_tag = name_str;
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_item {
                        let text = e.unescape().unwrap_or_default().to_string();
                        match current_tag.as_str() {
                            "title" => current_title.push_str(&text),
                            "link" => current_link.push_str(&text),
                            "pubDate" => current_pub_date.push_str(&text),
                            _ => {}
                        }
                    }
                }
                Ok(Event::CData(e)) => {
                    if in_item {
                        let text = String::from_utf8_lossy(e.into_inner().as_ref()).to_string();
                        match current_tag.as_str() {
                            "title" => current_title.push_str(&text),
                            "link" => current_link.push_str(&text),
                            "pubDate" => current_pub_date.push_str(&text),
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name_str = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    if name_str == "item" {
                        in_item = false;
                        if !current_title.is_empty() && !current_link.is_empty() {
                            let id = current_link
                                .trim_end_matches('/')
                                .split('/')
                                .last()
                                .unwrap_or("unknown")
                                .to_string();
                            
                            let parsed_date = chrono::DateTime::parse_from_rfc2822(&current_pub_date)
                                .map(|dt| dt.with_timezone(&chrono::Utc))
                                .ok();

                            items_to_insert.push((id, current_title.clone(), current_link.clone(), parsed_date));
                        }
                    }
                    if in_item {
                        current_tag.clear();
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    tracing::error!("Error parsing RSS XML: {:?}", e);
                    break;
                }
                _ => {}
            }
        }

        let mut oldest_date: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut total_updated = 0;
        let provider_name = self.name();

        let mut tx = pool.begin().await?;
        for (id, title, post_url, pub_date) in items_to_insert {
            if let Some(date) = pub_date {
                if oldest_date.is_none() || date < oldest_date.unwrap() {
                    oldest_date = Some(date);
                }
            }

            let pub_date_str = pub_date.map(|d| d.to_rfc3339());

            let updated = sqlx::query(
                r#"
                INSERT INTO games (id, provider, title, post_url, published_at, is_indexed)
                VALUES (?1, ?2, ?3, ?4, ?5, 0)
                ON CONFLICT(id) DO UPDATE SET 
                    is_indexed = CASE 
                        WHEN excluded.published_at > COALESCE(games.published_at, '') THEN 0 
                        ELSE games.is_indexed 
                    END,
                    published_at = excluded.published_at
                "#
            )
            .bind(&id)
            .bind(provider_name)
            .bind(&title)
            .bind(&post_url)
            .bind(pub_date_str)
            .execute(&mut *tx)
            .await?;

            if updated.rows_affected() > 0 {
                total_updated += 1;
            }
        }
        tx.commit().await?;

        tracing::info!("RSS sync complete. Total items updated/inserted: {}", total_updated);
        Ok(oldest_date)
    }
}
