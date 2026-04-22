use tokio::net::UdpSocket;
use std::time::Duration;
use tokio::time::timeout;

pub struct ScrapeResult {
    pub seeders: u32,
    pub leechers: u32,
}

pub async fn scrape_tracker(info_hash: &[u8; 20], tracker_addr: &str) -> Result<ScrapeResult, Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!("Connecting to tracker: {}", tracker_addr);
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(tracker_addr).await?;

    let transaction_id: u32 = rand::random();
    
    // 1. Connect
    let mut connect_packet = Vec::with_capacity(16);
    connect_packet.extend_from_slice(&0x41727101980u64.to_be_bytes()); // magic id
    connect_packet.extend_from_slice(&0u32.to_be_bytes()); // action 0: connect
    connect_packet.extend_from_slice(&transaction_id.to_be_bytes());

    let mut connection_id = [0u8; 8];
    let mut success = false;
    
    // Try connect up to 2 times
    for _ in 0..2 {
        socket.send(&connect_packet).await?;

        let mut buf = [0u8; 1024];
        if let Ok(Ok(n)) = timeout(Duration::from_secs(3), socket.recv(&mut buf)).await {
            if n >= 16 && buf[0..4] == 0u32.to_be_bytes() && buf[4..8] == transaction_id.to_be_bytes() {
                connection_id.copy_from_slice(&buf[8..16]);
                success = true;
                break;
            }
        }
    }

    if !success {
        return Err("Tracker connect timeout".into());
    }

    // 2. Scrape
    let scrape_transaction_id: u32 = rand::random();
    let mut scrape_packet = Vec::with_capacity(36);
    scrape_packet.extend_from_slice(&connection_id);
    scrape_packet.extend_from_slice(&2u32.to_be_bytes()); // action 2: scrape
    scrape_packet.extend_from_slice(&scrape_transaction_id.to_be_bytes());
    scrape_packet.extend_from_slice(info_hash);

    let mut seeders = 0;
    let mut leechers = 0;
    success = false;

    // Try scrape up to 2 times
    for _ in 0..2 {
        socket.send(&scrape_packet).await?;

        let mut buf = [0u8; 1024];
        if let Ok(Ok(n)) = timeout(Duration::from_secs(3), socket.recv(&mut buf)).await {
            if n >= 20 && buf[0..4] == 2u32.to_be_bytes() && buf[4..8] == scrape_transaction_id.to_be_bytes() {
                seeders = u32::from_be_bytes(buf[8..12].try_into().unwrap());
                leechers = u32::from_be_bytes(buf[16..20].try_into().unwrap());
                success = true;
                break;
            }
        }
    }

    if !success {
        return Err("Tracker scrape timeout".into());
    }

    Ok(ScrapeResult { seeders, leechers })
}
