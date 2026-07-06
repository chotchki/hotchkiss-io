//! The proof-of-work challenge kernel (CX.4) — pure crypto, no I/O.
//!
//! Design + rationale: `docs/greylist-challenge-design.md`. Three pieces:
//! 1. [`image_digest`] — the slow image chain (the client's actual work).
//! 2. [`derive_seed`] / [`compute_answer`] / [`verify_answer`] — the STATELESS signed challenge.
//! 3. [`mint_clearance`] / [`verify_clearance`] — the bearer clearance cookie.
//!
//! ## Client-parity contract (the browser JS in CX.6 MUST match byte-for-byte)
//! - Pixels are the raw RGBA buffer the server ships, row-major, 4 bytes each, forced opaque.
//! - Chain init `h = IV` = 32 zero bytes.
//! - For each pixel `p` (4 bytes) in order: `h = SHA256(p ‖ h)` (pixel bytes FIRST, then the
//!   32-byte prior hash), and every `h` is retained.
//! - Final digest = `SHA256(h[N-1] ‖ h[N-2] ‖ … ‖ h[0])` — the retained hashes concatenated in
//!   REVERSE order and hashed once (this is what forces holding the whole array in memory).
//! - `answer = HMAC-SHA256(key = seed, msg = image_digest)`, where `seed` is the 32 bytes the
//!   server handed out. SHA-256 + HMAC-SHA256 are standard, so any correct impl agrees.
//! The [`image_digest_kat`] test pins a known-answer vector the JS can be validated against.

use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Freshness window for a challenge answer. Kept SHORT because the stateless design has no
/// single-use marker, so this window IS the replay-sharing bound (design doc); it only has to
/// clear a ~1-2s machine solve plus network.
pub const FRESHNESS_WINDOW: Duration = Duration::from_secs(120);

/// Chain init — 32 zero bytes (kept trivial so the client JS can't get it subtly wrong; the
/// real binding is the server-key seed HMAC, not this).
const CHAIN_IV: [u8; 32] = [0u8; 32];

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut d = Sha256::new();
    for p in parts {
        d.update(p);
    }
    d.finalize().into()
}

/// HMAC-SHA256, mirroring the codebase's openssl HMAC pattern (`media::media_url_key`).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> Result<[u8; 32]> {
    use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
    let pkey = PKey::hmac(key).context("building HMAC key")?;
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey).context("HMAC signer")?;
    signer.update(msg).context("HMAC update")?;
    let mac = signer.sign_to_vec().context("HMAC sign")?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&mac);
    Ok(out)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && openssl::memcmp::eq(a, b)
}

/// The slow image chain: `h[i] = SHA256(pixel[i] ‖ h[i-1])` retaining every `h[i]`, then a
/// single SHA256 over the retained hashes in REVERSE order. Deterministic, so identical bytes
/// give an identical digest — and a one-pixel change avalanches the whole thing. See the
/// client-parity contract above. `rgba.len()` is assumed a multiple of 4 (a real RGBA buffer).
pub fn image_digest(rgba: &[u8]) -> [u8; 32] {
    let mut h = CHAIN_IV;
    let mut store: Vec<[u8; 32]> = Vec::with_capacity(rgba.len() / 4);
    for pixel in rgba.chunks_exact(4) {
        h = sha256(&[pixel, &h]);
        store.push(h);
    }
    let mut fold = Sha256::new();
    for hh in store.iter().rev() {
        fold.update(hh);
    }
    fold.finalize().into()
}

/// A short content-hash tag for the current toll image (`digest_version`). Bound into the seed,
/// so a token issued against one deploy's art fails cleanly after the art changes.
pub fn version_tag(rgba: &[u8]) -> String {
    to_hex(&sha256(&[rgba])[..6])
}

/// The three client-echoed fields the seed is derived from. Recomputing the seed from these
/// integrity-protects all three: tamper any and the seed (and therefore the answer) diverges.
pub struct ChallengeParams<'a> {
    pub inner_seed: &'a [u8],
    pub ts: i64,
    pub version: &'a str,
}

/// `seed = HMAC(server_key, inner_seed ‖ ts ‖ version)`. Unforgeable (needs `server_key`) and
/// un-post-dateable (`ts` is inside the MAC). The server hands `seed` to the client; the client
/// echoes the three inputs so the server can re-derive it on verify.
pub fn derive_seed(server_key: &[u8], p: &ChallengeParams) -> Result<[u8; 32]> {
    let mut msg = Vec::with_capacity(p.inner_seed.len() + 8 + p.version.len());
    msg.extend_from_slice(p.inner_seed);
    msg.extend_from_slice(&p.ts.to_le_bytes());
    msg.extend_from_slice(p.version.as_bytes());
    hmac_sha256(server_key, &msg)
}

/// `answer = HMAC(seed, image_digest)` — the client's proof it recovered the image and ran the
/// chain. The server recomputes it in [`verify_answer`].
pub fn compute_answer(seed: &[u8; 32], image_digest: &[u8; 32]) -> Result<[u8; 32]> {
    hmac_sha256(seed, image_digest)
}

/// Verify a submitted answer end to end: freshness window, seed re-derivation, and a
/// constant-time compare against the recomputed expected answer. Returns `Ok(true)` ONLY on a
/// full match — never accepts a merely well-formed value (the CVE-2025-24369 class), because it
/// always recomputes and compares. `image_digest` is THIS version's cached digest; `now` and the
/// window are injected for testability.
pub fn verify_answer(
    server_key: &[u8],
    p: &ChallengeParams,
    image_digest: &[u8; 32],
    submitted_answer: &[u8],
    now: i64,
    window: Duration,
) -> Result<bool> {
    // Issued in the past, and not older than the window. A future ts is rejected outright.
    if p.ts > now || now - p.ts > window.as_secs() as i64 {
        return Ok(false);
    }
    let seed = derive_seed(server_key, p)?;
    let expected = compute_answer(&seed, image_digest)?;
    Ok(ct_eq(&expected, submitted_answer))
}

/// Mint a bearer clearance token `"<expiry_unix>.<hex_mac>"`, `mac = HMAC(server_key, expiry)`.
/// Deliberately NOT IP-bound (mobile IPs churn — design doc). The caller sets
/// HttpOnly+Secure+SameSite on the `Set-Cookie`.
pub fn mint_clearance(server_key: &[u8], expiry: i64) -> Result<String> {
    let mac = hmac_sha256(server_key, &expiry.to_le_bytes())?;
    Ok(format!("{expiry}.{}", to_hex(&mac)))
}

/// Verify a clearance token: re-derive the MAC over the CLAIMED expiry, constant-time compare,
/// and require `expiry > now`. Any malformed / expired / forged token → `false`.
pub fn verify_clearance(server_key: &[u8], token: &str, now: i64) -> bool {
    let Some((exp_s, mac_hex)) = token.split_once('.') else {
        return false;
    };
    let Ok(expiry) = exp_s.parse::<i64>() else {
        return false;
    };
    if expiry <= now {
        return false;
    }
    let Ok(expected) = hmac_sha256(server_key, &expiry.to_le_bytes()) else {
        return false;
    };
    ct_eq(mac_hex.as_bytes(), to_hex(&expected).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"test-server-key-crypto-keys-id-4";

    fn params<'a>(inner: &'a [u8], ts: i64, ver: &'a str) -> ChallengeParams<'a> {
        ChallengeParams { inner_seed: inner, ts, version: ver }
    }

    #[test]
    fn image_digest_is_deterministic_and_avalanches() {
        let a = vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255];
        assert_eq!(image_digest(&a), image_digest(&a), "same bytes -> same digest");

        let mut b = a.clone();
        b[5] ^= 1; // flip one bit of one pixel
        assert_ne!(image_digest(&a), image_digest(&b), "one-pixel change avalanches");
    }

    #[test]
    fn image_digest_kat() {
        // Known-answer vector — the client JS chain is validated against this exact value.
        // Two opaque pixels: (1,2,3) and (4,5,6).
        let rgba = [1u8, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(
            to_hex(&image_digest(&rgba)),
            "152deeca4fcc6400042c4b8cbe5132af610ef2930edad4aa746d5a3e6b6e2d24"
        );
    }

    #[test]
    fn answer_roundtrips_and_verifies() {
        let p = params(b"inner-seed-16byt", 1000, "abc123");
        let digest = image_digest(&[1, 2, 3, 255, 4, 5, 6, 255]);
        let seed = derive_seed(KEY, &p).unwrap();
        let answer = compute_answer(&seed, &digest).unwrap();
        assert!(verify_answer(KEY, &p, &digest, &answer, 1030, FRESHNESS_WINDOW).unwrap());
    }

    #[test]
    fn verify_rejects_a_wellformed_but_wrong_answer() {
        // CVE-2025-24369 class: a valid-shaped 32-byte answer that wasn't recomputed must fail.
        let p = params(b"inner-seed-16byt", 1000, "abc123");
        let digest = image_digest(&[9, 9, 9, 255]);
        let bogus = [0u8; 32];
        assert!(!verify_answer(KEY, &p, &digest, &bogus, 1005, FRESHNESS_WINDOW).unwrap());
    }

    #[test]
    fn verify_rejects_stale_or_future_timestamps() {
        let p = params(b"inner-seed-16byt", 1000, "abc123");
        let digest = image_digest(&[1, 2, 3, 255]);
        let seed = derive_seed(KEY, &p).unwrap();
        let answer = compute_answer(&seed, &digest).unwrap();
        // Too old (> 120s).
        assert!(!verify_answer(KEY, &p, &digest, &answer, 1000 + 121, FRESHNESS_WINDOW).unwrap());
        // Future issue time.
        assert!(!verify_answer(KEY, &p, &digest, &answer, 999, FRESHNESS_WINDOW).unwrap());
        // Within window still passes.
        assert!(verify_answer(KEY, &p, &digest, &answer, 1000 + 120, FRESHNESS_WINDOW).unwrap());
    }

    #[test]
    fn tampering_any_echoed_field_breaks_the_answer() {
        let digest = image_digest(&[1, 2, 3, 255]);
        let seed = derive_seed(KEY, &params(b"inner-seed-16byt", 1000, "abc123")).unwrap();
        let answer = compute_answer(&seed, &digest).unwrap();
        // Same answer, but the client claims a different version / ts / inner_seed → mismatch.
        assert!(!verify_answer(KEY, &params(b"inner-seed-16byt", 1000, "DIFF"), &digest, &answer, 1010, FRESHNESS_WINDOW).unwrap());
        assert!(!verify_answer(KEY, &params(b"inner-seed-16byt", 1001, "abc123"), &digest, &answer, 1010, FRESHNESS_WINDOW).unwrap());
        assert!(!verify_answer(KEY, &params(b"OTHER-seed-16byt", 1000, "abc123"), &digest, &answer, 1010, FRESHNESS_WINDOW).unwrap());
    }

    #[test]
    fn clearance_roundtrips_and_rejects_forgery_and_expiry() {
        let token = mint_clearance(KEY, 5000).unwrap();
        assert!(verify_clearance(KEY, &token, 4000), "valid + unexpired");
        assert!(!verify_clearance(KEY, &token, 5001), "expired");
        assert!(!verify_clearance(b"wrong-key-wrong-key-wrong-key-42", &token, 4000), "wrong key");

        // Attacker bumps the expiry but can't recompute the MAC.
        let forged = format!("9999999999.{}", &token.split_once('.').unwrap().1);
        assert!(!verify_clearance(KEY, &forged, 4000), "expiry tamper fails the MAC");
        // Garbage.
        assert!(!verify_clearance(KEY, "not-a-token", 4000));
        assert!(!verify_clearance(KEY, "5000.zzzz", 4000));
    }
}
