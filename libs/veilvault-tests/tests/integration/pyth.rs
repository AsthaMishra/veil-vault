/// Mock Pyth PriceUpdateV2 account injected directly into LiteSVM.
///
/// Layout matches pyth-solana-receiver-sdk 0.3.1 PriceUpdateV2 Anchor account:
///   [0..8]    discriminator  = sha256("account:PriceUpdateV2")[..8]
///   [8..40]   write_authority: Pubkey (32 bytes)
///   [40]      verification_level: u8 — 1 = Full
///   [41..73]  price_message.feed_id: [u8; 32]
///   [73..81]  price_message.price: i64 LE
///   [81..89]  price_message.conf: u64 LE
///   [89..93]  price_message.exponent: i32 LE
///   [93..101] price_message.publish_time: i64 LE
///   [101..109]price_message.prev_publish_time: i64 LE
///   [109..117]price_message.ema_price: i64 LE
///   [117..125]price_message.ema_conf: u64 LE
///   [125..133]posted_slot: u64 LE
use litesvm::LiteSVM;
use sha2::{Digest, Sha256};
use solana_sdk::{account::Account, pubkey::Pubkey};

pub fn create_price_account(svm: &mut LiteSVM, price_usd: f64, publish_time: i64) -> Pubkey {
    let key = Pubkey::new_unique();
    let data = build_price_data(price_usd, publish_time);
    // owner can be any pubkey — UncheckedAccount skips owner validation
    let owner: Pubkey = "pythWSnswVUSvBkm3oFa2BgHADeLpV19H9dLEYMkMT"
        .parse()
        .unwrap_or_default();
    let account = Account {
        lamports: u32::MAX as u64,
        data,
        owner,
        executable: false,
        rent_epoch: 0,
    };
    svm.set_account(key, account).unwrap();
    key
}

/// Update an existing price account with a new price and timestamp.
/// Call this before `refresh_reserve` in tests that advance the clock.
pub fn update_price(svm: &mut LiteSVM, key: Pubkey, price_usd: f64, publish_time: i64) {
    let mut account = svm.get_account(&key).expect("pyth account not found");
    account.data = build_price_data(price_usd, publish_time);
    svm.set_account(key, account).unwrap();
}

fn build_price_data(price_usd: f64, publish_time: i64) -> Vec<u8> {
    // Pyth uses exponent = -8 → price_i64 = price_usd × 10^8
    let exponent: i32 = -8;
    let price_i64 = (price_usd * 1e8) as i64;
    // confidence = 0.1% of price — well below the 2% threshold the program checks
    let conf: u64 = (price_i64 as u64) / 1_000;

    let mut data = Vec::with_capacity(133);

    // discriminator
    let mut h = Sha256::new();
    h.update(b"account:PriceUpdateV2");
    data.extend_from_slice(&h.finalize()[..8]);

    // write_authority (32 zero bytes — unused)
    data.extend_from_slice(&[0u8; 32]);

    // verification_level: Full = variant 1, no fields → 1 byte
    data.push(1u8);

    // price_message.feed_id (32 zero bytes)
    data.extend_from_slice(&[0u8; 32]);
    // price_message.price
    data.extend_from_slice(&price_i64.to_le_bytes());
    // price_message.conf
    data.extend_from_slice(&conf.to_le_bytes());
    // price_message.exponent
    data.extend_from_slice(&exponent.to_le_bytes());
    // price_message.publish_time
    data.extend_from_slice(&publish_time.to_le_bytes());
    // price_message.prev_publish_time
    data.extend_from_slice(&publish_time.to_le_bytes());
    // price_message.ema_price
    data.extend_from_slice(&price_i64.to_le_bytes());
    // price_message.ema_conf
    data.extend_from_slice(&conf.to_le_bytes());

    // posted_slot
    data.extend_from_slice(&1u64.to_le_bytes());

    data
}
