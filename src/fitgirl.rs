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
            let mut has_next = false;
            let mut parsed_any = false;
            
            {
                let document = Html::parse_document(&html_content);
                let ul_selector = Selector::parse("ul.lcp_catlist li a").unwrap();
                let next_selector = Selector::parse("ul.lcp_paginator li a[title='Next']").unwrap();

                has_next = document.select(&next_selector).next().is_some();
                parsed_any = document.select(&ul_selector).next().is_some();

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
            }

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

    async fn fetch_metadata(&self, post_url: &str) -> Result<(Option<String>, Option<i64>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Deep fetching metadata for: {}", post_url);
        
        let res = self.client.get(post_url).send().await?;
        let html_content = res.text().await?;
        let document = Html::parse_document(&html_content);
        
        let mut magnet_link = None;
        let mut size_bytes = None;
        let mut published_at = None;

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
            
            if let Some(caps) = regex::Regex::new(r"(?i)(?:repack\s+)?size[:\s]+(?:from\s+)?([\d.]+)\s*([KMGT]B)")
                .unwrap()
                .captures(&full_text) 
            {
                let val: f64 = caps.get(1).unwrap().as_str().parse().unwrap_or(0.0);
                let unit = caps.get(2).unwrap().as_str().to_uppercase();
                
                let multiplier = match unit.as_str() {
                    "KB" => 1024,
                    "MB" => 1024 * 1024,
                    "GB" => 1024 * 1024 * 1024,
                    "TB" => 1024 * 1024 * 1024 * 1024,
                    _ => 1,
                };
                
                size_bytes = Some((val * multiplier as f64) as i64);
            }
        }

        Ok((magnet_link, size_bytes, published_at))
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

            let updated = sqlx::query!(
                r#"
                INSERT INTO games (id, provider, title, post_url, is_indexed)
                VALUES (?1, ?2, ?3, ?4, 0)
                ON CONFLICT(id) DO UPDATE SET is_indexed = 0
                "#,
                id,
                provider_name,
                title,
                post_url
            )
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
