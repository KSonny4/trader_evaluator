use common::types::DiscoverySource;
use std::collections::HashMap;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DiscoveredWallet {
    pub proxy_wallet: String,
    pub discovered_from: DiscoverySource,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct HolderWallet {
    pub proxy_wallet: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TradeWallet {
    pub proxy_wallet: String,
}

#[allow(dead_code)]
pub fn discover_wallets_for_market(
    holders: &[HolderWallet],
    trades: &[TradeWallet],
    min_trades_in_market: u32,
) -> Vec<DiscoveredWallet> {
    // Earliest tag wins (HOLDER evaluated before TRADER_RECENT).
    let mut by_wallet: HashMap<String, DiscoverySource> = HashMap::new();

    for h in holders {
        by_wallet
            .entry(h.proxy_wallet.clone())
            .or_insert(DiscoverySource::Holder);
    }

    // Count trades per wallet in this market; use it for pruning.
    let mut trade_counts: HashMap<&str, u32> = HashMap::new();
    for t in trades {
        *trade_counts.entry(t.proxy_wallet.as_str()).or_insert(0) += 1;
    }

    for t in trades {
        if trade_counts
            .get(t.proxy_wallet.as_str())
            .copied()
            .unwrap_or(0)
            < min_trades_in_market
        {
            continue;
        }
        by_wallet
            .entry(t.proxy_wallet.clone())
            .or_insert(DiscoverySource::TraderRecent);
    }

    let mut out: Vec<DiscoveredWallet> = by_wallet
        .into_iter()
        .map(|(proxy_wallet, discovered_from)| DiscoveredWallet {
            proxy_wallet,
            discovered_from,
        })
        .collect();

    out.sort_by(|a, b| a.proxy_wallet.cmp(&b.proxy_wallet));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_wallets_for_market_dedup_and_filter() {
        // Two holders; one of them also appears in trades.
        let holders = vec![
            HolderWallet {
                proxy_wallet: "0xholder1".to_string(),
            },
            HolderWallet {
                proxy_wallet: "0xdup".to_string(),
            },
        ];

        // Trades contain: dup wallet (3 trades), trader-only wallet (1 trade), and another trader (2 trades).
        let trades = vec![
            TradeWallet {
                proxy_wallet: "0xdup".to_string(),
            },
            TradeWallet {
                proxy_wallet: "0xdup".to_string(),
            },
            TradeWallet {
                proxy_wallet: "0xdup".to_string(),
            },
            TradeWallet {
                proxy_wallet: "0xtrader1".to_string(),
            },
            TradeWallet {
                proxy_wallet: "0xtrader2".to_string(),
            },
            TradeWallet {
                proxy_wallet: "0xtrader2".to_string(),
            },
        ];

        // Require at least 2 trades in this market to keep a trader-discovered wallet.
        let discovered = discover_wallets_for_market(&holders, &trades, 2);

        // dup should be tagged HOLDER (earliest source wins).
        let dup = discovered
            .iter()
            .find(|w| w.proxy_wallet == "0xdup")
            .unwrap();
        assert_eq!(dup.discovered_from.as_str(), "HOLDER");

        // holder1 should be present as HOLDER.
        assert!(discovered.iter().any(|w| w.proxy_wallet == "0xholder1"));

        // trader2 has 2 trades -> included.
        let trader2 = discovered
            .iter()
            .find(|w| w.proxy_wallet == "0xtrader2")
            .unwrap();
        assert_eq!(trader2.discovered_from.as_str(), "TRADER_RECENT");

        // trader1 has only 1 trade -> filtered out.
        assert!(!discovered.iter().any(|w| w.proxy_wallet == "0xtrader1"));
    }
}
