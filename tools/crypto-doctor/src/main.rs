//! crypto-doctor: look at, and repair, the bridge's encryption store while the bridge is STOPPED.
//!
//!   crypto-doctor show <session-file> <store-dir>...
//!       Prints the device key (ed25519) and the cross-signing keys held by each store directory
//!       (e.g. data/db/ep58bOF). Compare the device key with the one Synapse holds (scripts/check-bridge-keys.sh).
//!
//!   crypto-doctor import-identity <from-session> <from-store-dir> <to-session> <to-store-dir>
//!       Copies the private cross-signing identity (master / self-signing / user-signing keys) from one
//!       store into another, so a store with the right device key (or a brand new device) also gets the
//!       right cross-signing keys. Each store is opened with the passphrase of its own session file
//!       (use the same file twice when both stores belong to the same session).
//!
//! <session-file> is the bridge's `session` file: only its store passphrase is used, and it is never printed.
use matrix_sdk_crypto::store::{types::Changes, CryptoStore};
use matrix_sdk_sqlite::SqliteCryptoStore;

fn passphrase(session_file: &str) -> String {
    let text = std::fs::read_to_string(session_file).unwrap_or_else(|e| die(&format!("cannot read {session_file}: {e}")));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| die(&format!("{session_file} is not valid JSON: {e}")));
    json["client_session"]["passphrase"]
        .as_str()
        .unwrap_or_else(|| die("no client_session.passphrase in the session file"))
        .to_string()
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

async fn open(dir: &str, pass: &str) -> SqliteCryptoStore {
    SqliteCryptoStore::open(dir, Some(pass)).await.unwrap_or_else(|e| die(&format!("cannot open {dir} (wrong passphrase, or not a store?): {e}")))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("show") if args.len() >= 4 => {
            let pass = passphrase(&args[2]);
            for dir in &args[3..] {
                let store = open(dir, &pass).await;
                match store.load_account().await {
                    Ok(Some(account)) => println!("{dir}\n   device key (ed25519): {}", account.identity_keys().ed25519.to_base64()),
                    _ => println!("{dir}\n   no account in this store"),
                }
                match store.load_identity().await {
                    Ok(Some(id)) => {
                        let status = id.status().await;
                        let master = id.master_public_key().await.and_then(|k| k.get_first_key().map(|x| x.to_base64()));
                        println!(
                            "   cross-signing: master={} self-signing={} user-signing={}  master key: {}",
                            status.has_master,
                            status.has_self_signing,
                            status.has_user_signing,
                            master.unwrap_or_default()
                        );
                    }
                    _ => println!("   cross-signing: none (the bridge can only trust devices one at a time)"),
                }
            }
        }
        Some("import-identity") if args.len() == 6 => {
            let from = open(&args[3], &passphrase(&args[2])).await;
            let to = open(&args[5], &passphrase(&args[4])).await;
            let id = from
                .load_identity()
                .await
                .unwrap_or_else(|e| die(&format!("cannot read the source identity: {e}")))
                .unwrap_or_else(|| die("the source store has no private cross-signing identity"));
            let changes = Changes { private_identity: Some(id), ..Default::default() };
            to.save_changes(changes).await.unwrap_or_else(|e| die(&format!("cannot write the target: {e}")));
            let back = to.load_identity().await.ok().flatten().unwrap_or_else(|| die("identity missing after the write"));
            let status = back.status().await;
            println!(
                "done: target now has master={} self-signing={} user-signing={}",
                status.has_master, status.has_self_signing, status.has_user_signing
            );
        }
        _ => {
            eprintln!("usage:\n  crypto-doctor show <session-file> <store-dir>...\n  crypto-doctor import-identity <from-session> <from-store-dir> <to-session> <to-store-dir>");
            std::process::exit(2);
        }
    }
}
