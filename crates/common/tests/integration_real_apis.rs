use common::polymarket::PolymarketClient;
use common::types::ApiTrade;

#[tokio::test]
#[ignore] // requires network
async fn test_fetch_real_markets_and_save_fixture() {
    let client = PolymarketClient::new(
        "https://data-api.polymarket.com",
        "https://gamma-api.polymarket.com",
    );
    let markets = client.fetch_gamma_markets(5, 0).await.unwrap();
    assert!(!markets.is_empty());

    std::fs::create_dir_all("tests/fixtures").unwrap();
    std::fs::write(
        "tests/fixtures/gamma_markets_live.json",
        serde_json::to_string_pretty(&markets).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
#[ignore] // requires network
async fn test_fetch_real_recent_trades_parses() {
    // Data API supports recent trades without a user filter.
    let body = reqwest::get("https://data-api.polymarket.com/trades?limit=5")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let trades: Vec<ApiTrade> = serde_json::from_str(&body).unwrap();
    assert!(!trades.is_empty());
}
