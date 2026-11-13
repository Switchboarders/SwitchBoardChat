use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::{Rng, RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

pub fn generate_token(length: usize) -> String {
    let charset: Vec<u8> = b"abcdefghijklmnopqrstuvwxyz0123456789".to_vec();
    let token: String = (0..length)
        .map(|_| {
            let idx = OsRng.gen_range(0..charset.len());
            charset[idx] as char
        })
        .collect();
    token
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();

    // Convert to hexadecimal string
    format!("{:x}", result)
}

pub fn hash_phone(phone: &str, secret_pepper: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(phone.as_bytes());
    hasher.update(secret_pepper.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ========================================
// PERSONAL DATA PROTECTION (Quebec Law 25)
// Caller phone numbers are never stored or logged in clear:
// - lookups use a keyed hash (`hash_did`), which cannot be reversed
// - the few places that must send an SMS keep the number encrypted (`encrypt_did`)
// - voip.ms passwords are stored encrypted (`seal_secret`)
// ========================================

/// Marks a secret stored encrypted (anything else is legacy plaintext, still readable)
const SEALED_PREFIX: &str = "enc:";

fn hash_secret() -> String {
    std::env::var("PHONE_HASH_SECRET").expect("PHONE_HASH_SECRET must be set")
}

/// Keyed hash of a phone number, fit for a SQLite integer column (63 bits).
/// Same number and same PHONE_HASH_SECRET always give the same value. Changing the secret
/// makes every previous hash (opt-outs included) unrecognizable.
pub fn hash_did(did: u64) -> i64 {
    let digest = Sha256::digest(format!("did:{did}:{}", hash_secret()).as_bytes());
    (u64::from_be_bytes(digest[0..8].try_into().unwrap()) & (i64::MAX as u64)) as i64
}

/// Keyed hash of a network address (see `web_chat::ip_key`), for the web chat block list: the
/// address itself is never stored. Same address and same PHONE_HASH_SECRET give the same value.
pub fn hash_ip(key: &str) -> String {
    format!("{:x}", Sha256::digest(format!("ip:{key}:{}", hash_secret()).as_bytes()))
}

/// HMAC-SHA256 (RFC 2104)
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Keyed MAC (hex) of a message, with the server secret: signs the web chat's block markers and
/// proof-of-work challenges so they cannot be forged.
pub fn web_mac(msg: &str) -> String {
    encode_hex(&hmac_sha256(hash_secret().as_bytes(), msg.as_bytes()))
}

/// Constant-time string comparison (for MACs)
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Short identifier for logs, so events can be correlated without writing the number
pub fn short_id(did: u64) -> String {
    format!("{:08x}", (hash_did(did) as u64) >> 32)
}

fn encryption_key() -> Key<Aes256Gcm> {
    let hex = std::env::var("PHONE_ENC_KEY")
        .expect("PHONE_ENC_KEY must be set (64 hex characters, e.g. `openssl rand -hex 32`)");
    let bytes = decode_hex(hex.trim()).expect("PHONE_ENC_KEY must be hexadecimal");
    assert!(bytes.len() == 32, "PHONE_ENC_KEY must be 64 hex characters (32 bytes)");
    *Key::<Aes256Gcm>::from_slice(&bytes)
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// AES-256-GCM. Output is nonce (12 bytes) followed by the ciphertext.
pub fn encrypt_bytes(plain: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(&encryption_key());
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut out = nonce.to_vec();
    out.extend(
        cipher
            .encrypt(Nonce::from_slice(&nonce), plain)
            .expect("encryption failed"),
    );
    out
}

pub fn decrypt_bytes(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 {
        return None;
    }
    let (nonce, ciphertext) = data.split_at(12);
    Aes256Gcm::new(&encryption_key())
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .ok()
}

pub fn encrypt_did(did: u64) -> Vec<u8> {
    encrypt_bytes(&did.to_be_bytes())
}

pub fn decrypt_did(data: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(decrypt_bytes(data)?.try_into().ok()?))
}

/// Encrypts a secret for storage in a text column (idempotent: already sealed stays as is)
pub fn seal_secret(plain: &str) -> String {
    if plain.starts_with(SEALED_PREFIX) {
        return plain.to_string();
    }
    format!("{SEALED_PREFIX}{}", encode_hex(&encrypt_bytes(plain.as_bytes())))
}

/// Reads a stored secret: decrypts it, or returns legacy plaintext unchanged
pub fn open_secret(stored: &str) -> String {
    let Some(hex) = stored.strip_prefix(SEALED_PREFIX) else {
        return stored.to_string();
    };
    decode_hex(hex)
        .and_then(|b| decrypt_bytes(&b))
        .and_then(|b| String::from_utf8(b).ok())
        .expect("stored secret cannot be decrypted: wrong PHONE_ENC_KEY?")
}

#[cfg(test)]
pub fn init_test_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        std::env::set_var("PHONE_HASH_SECRET", "test-secret");
        std::env::set_var(
            "PHONE_ENC_KEY",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn did_hash_is_stable_positive_and_not_the_number() {
        init_test_env();
        let h = hash_did(4185551234);
        assert_eq!(h, hash_did(4185551234));
        assert_ne!(h, hash_did(4185551235));
        assert!(h >= 0);
        assert!(!short_id(4185551234).contains("4185551234"));
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        // test case 1
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(encode_hex(&mac), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        // test case 2
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(encode_hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // test case 6: key longer than the block size
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(encode_hex(&mac), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn constant_time_compare_works() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn ip_hash_is_keyed_and_stable() {
        init_test_env();
        let h = hash_ip("203.0.113.9");
        assert_eq!(h, hash_ip("203.0.113.9"));
        assert_ne!(h, hash_ip("203.0.113.10"));
        assert_eq!(h.len(), 64);
        assert!(!h.contains("203"));
    }

    #[test]
    fn did_round_trips_through_encryption() {
        init_test_env();
        let a = encrypt_did(4185551234);
        assert_eq!(decrypt_did(&a), Some(4185551234));
        // random nonce: same plaintext, different ciphertext
        assert_ne!(a, encrypt_did(4185551234));
        // tampering is detected
        let mut bad = a.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(decrypt_did(&bad), None);
        assert_eq!(decrypt_did(&[1, 2, 3]), None);
    }

    #[test]
    fn secrets_are_sealed_and_legacy_plaintext_still_reads() {
        init_test_env();
        let sealed = seal_secret("hunter2");
        assert!(sealed.starts_with("enc:") && !sealed.contains("hunter2"));
        assert_eq!(seal_secret(&sealed), sealed);
        assert_eq!(open_secret(&sealed), "hunter2");
        assert_eq!(open_secret("legacy-plain"), "legacy-plain");
    }
}
