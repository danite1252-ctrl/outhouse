//! Two-factor sign-in with an authenticator app (Google Authenticator,
//! Microsoft Authenticator, 1Password, Aegis, ...):
//!
//! - TOTP codes (RFC 6238: HMAC-SHA1, 6 digits, 30-second steps, one step
//!   either side allowed for clock drift, and no step accepted twice);
//! - the secret encrypted at rest with the server's `AUTH_SECRET_KEY`
//!   (AES-256-GCM, bound to the account), so a copy of the database alone
//!   doesn't give anyone the codes;
//! - one-time recovery codes (only hashes kept);
//! - the QR code for the app to scan (drawn here as SVG, nothing external).

use std::path::Path;
use std::sync::OnceLock;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine;
use ring::{aead, hmac};
use sha2::{Digest, Sha256};

pub const ISSUER: &str = "Outhouse";
pub const STEP_SECS: u64 = 30;
pub const DIGITS: u32 = 6;
/// Steps either side of now that still count (clock drift on phones).
pub const WINDOW: i64 = 1;
pub const RECOVERY_CODES: usize = 10;
/// Bytes of randomness in a secret (160 bits, as RFC 4226 recommends).
const SECRET_BYTES: usize = 20;

// ---------- base32 (RFC 4648, what authenticator apps use) ----------

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// (For checking secrets in tests and by hand.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in text.chars() {
        if c == '=' || c == ' ' || c == '-' {
            continue;
        }
        let v = B32
            .iter()
            .position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

// ---------- codes ----------

/// HOTP (RFC 4226) with `digits` digits.
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let h = tag.as_ref();
    let offset = (h[h.len() - 1] & 0x0f) as usize;
    let binary = ((h[offset] as u32 & 0x7f) << 24)
        | ((h[offset + 1] as u32) << 16)
        | ((h[offset + 2] as u32) << 8)
        | h[offset + 3] as u32;
    binary % 10u32.pow(digits)
}

/// The step (30-second slot) a moment falls in.
pub fn step_at(unix_secs: u64) -> i64 {
    (unix_secs / STEP_SECS) as i64
}

pub fn code_for_step(secret: &[u8], step: i64) -> String {
    format!(
        "{:0width$}",
        hotp(secret, step as u64, DIGITS),
        width = DIGITS as usize
    )
}

/// Just the digits of what someone typed ("123 456" → "123456").
pub fn clean_code(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Which step `code` is right for, within the window around `now`, and
/// only if that step is newer than `last_step` (a code never works twice).
pub fn check_code(secret: &[u8], code: &str, now_unix: u64, last_step: i64) -> Option<i64> {
    let code = clean_code(code);
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let now = step_at(now_unix);
    let mut found = None;
    for step in (now - WINDOW)..=(now + WINDOW) {
        // Compare every candidate (no early exit on a match).
        if constant_eq(code_for_step(secret, step).as_bytes(), code.as_bytes()) && step > last_step
        {
            found = Some(step);
        }
    }
    found
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn new_secret() -> Vec<u8> {
    let mut s = vec![0u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut s);
    s
}

fn uri_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The `otpauth://` link authenticator apps read from the QR code.
pub fn otpauth_uri(username: &str, secret_b32: &str) -> String {
    format!(
        "otpauth://totp/{issuer}:{user}?secret={secret_b32}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        issuer = uri_escape(ISSUER),
        user = uri_escape(username),
    )
}

/// The QR code for `data`, as an SVG picture (black on white, with the
/// quiet zone scanners need).
pub fn qr_svg(data: &str) -> Option<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::with_error_correction_level(data, qrcode::EcLevel::M).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(220, 220)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build(),
    )
}

// ---------- recovery codes ----------

/// Letters and digits that can't be mistaken for each other.
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Ten new codes like `k7m2p-x9q4t` (50 bits each).
pub fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let raw = crate::auth::random_code(10, RECOVERY_ALPHABET);
            format!("{}-{}", &raw[..5], &raw[5..])
        })
        .collect()
}

/// What someone typed, as stored: lower case, no spaces or dashes.
pub fn normalize_recovery(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub fn recovery_hash(code: &str) -> String {
    let digest = Sha256::digest(format!("outhouse-recovery:{}", normalize_recovery(code)));
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A recovery code rather than a 6-digit code?
pub fn looks_like_recovery(code: &str) -> bool {
    normalize_recovery(code).len() == 10 && !clean_code(code).chars().all(|c| c.is_ascii_digit())
}

// ---------- the secret at rest ----------

/// Where the encryption key came from (shown on Server status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `AUTH_SECRET_KEY` (the way it should be).
    Env,
    /// A key file the server made in the upload folder because
    /// `AUTH_SECRET_KEY` wasn't set.
    File,
    /// No key at all: two-factor can't be turned on.
    Missing,
}

impl KeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Env => "env",
            KeySource::File => "file",
            KeySource::Missing => "missing",
        }
    }
}

struct Keyring {
    key: Option<aead::LessSafeKey>,
    /// The key file's key, when AUTH_SECRET_KEY was set after the server had
    /// already made one: secrets sealed with it still open (and new ones
    /// use AUTH_SECRET_KEY).
    older: Option<aead::LessSafeKey>,
    source: KeySource,
}

static KEYRING: OnceLock<Keyring> = OnceLock::new();

fn derive(material: &[u8]) -> aead::LessSafeKey {
    let mut h = Sha256::new();
    h.update(b"outhouse-auth-secret-key-v1:");
    h.update(material);
    let bytes = h.finalize();
    aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &bytes).expect("32-byte AES key"),
    )
}

/// Loads the key: `AUTH_SECRET_KEY` (32+ characters), otherwise a key file
/// kept next to the uploads (made on first start). Call once at start-up.
pub fn init(upload_dir: &Path) -> KeySource {
    let ring = KEYRING.get_or_init(|| {
        if let Some(k) = std::env::var("AUTH_SECRET_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            if k.chars().count() >= 32 {
                let older = std::fs::read_to_string(upload_dir.join(KEY_FILE))
                    .ok()
                    .map(|t| t.trim().to_string())
                    .filter(|t| t.len() >= 32)
                    .map(|t| derive(t.as_bytes()));
                return Keyring {
                    key: Some(derive(k.as_bytes())),
                    older,
                    source: KeySource::Env,
                };
            }
            tracing::error!(
                "AUTH_SECRET_KEY is too short (it needs 32+ characters; deploy/setup.sh makes one). Using the key file instead."
            );
        }
        match file_key(upload_dir) {
            Ok(material) => {
                tracing::warn!(
                    "AUTH_SECRET_KEY isn't set: two-factor secrets are encrypted with a key file in the upload folder. Set AUTH_SECRET_KEY (deploy/README.md)."
                );
                Keyring {
                    key: Some(derive(&material)),
                    older: None,
                    source: KeySource::File,
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "no AUTH_SECRET_KEY and no key file: two-factor can't be turned on");
                Keyring {
                    key: None,
                    older: None,
                    source: KeySource::Missing,
                }
            }
        }
    });
    ring.source
}

const KEY_FILE: &str = ".auth-secret-key";

fn file_key(upload_dir: &Path) -> std::io::Result<Vec<u8>> {
    let path = upload_dir.join(KEY_FILE);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let t = existing.trim();
        if t.len() >= 32 {
            return Ok(t.as_bytes().to_vec());
        }
    }
    std::fs::create_dir_all(upload_dir)?;
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let text: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&path)?;
        f.write_all(text.as_bytes())?;
    }
    Ok(text.into_bytes())
}

pub fn key_source() -> KeySource {
    KEYRING
        .get()
        .map(|k| k.source)
        .unwrap_or(KeySource::Missing)
}

fn key() -> Option<&'static aead::LessSafeKey> {
    KEYRING.get().and_then(|k| k.key.as_ref())
}

fn aad(user_id: i64) -> Vec<u8> {
    format!("outhouse-totp:{user_id}").into_bytes()
}

/// `v1.<base64(nonce || ciphertext)>`, bound to the account.
pub fn encrypt(user_id: i64, plain: &[u8]) -> Option<String> {
    encrypt_with(key()?, user_id, plain)
}

pub fn decrypt(user_id: i64, sealed: &str) -> Option<Vec<u8>> {
    decrypt_with(key()?, user_id, sealed).or_else(|| {
        let older = KEYRING.get()?.older.as_ref()?;
        decrypt_with(older, user_id, sealed)
    })
}

fn encrypt_with(key: &aead::LessSafeKey, user_id: i64, plain: &[u8]) -> Option<String> {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut buf = plain.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad(user_id)),
        &mut buf,
    )
    .ok()?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&buf);
    Some(format!(
        "v1.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(out)
    ))
}

fn decrypt_with(key: &aead::LessSafeKey, user_id: i64, sealed: &str) -> Option<Vec<u8>> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sealed.strip_prefix("v1.")?)
        .ok()?;
    if raw.len() < 12 + 16 {
        return None;
    }
    let (nonce, rest) = raw.split_at(12);
    let mut buf = rest.to_vec();
    let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
    let plain = key
        .open_in_place(nonce, aead::Aad::from(aad(user_id)), &mut buf)
        .ok()?;
    Some(plain.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 appendix B (SHA-1, 8 digits).
        for (t, want) in [
            (59u64, 94287082u32),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ] {
            assert_eq!(hotp(RFC_SECRET, t / STEP_SECS, 8), want, "time {t}");
        }
        // And the 6-digit codes apps show are the last six of those.
        assert_eq!(code_for_step(RFC_SECRET, step_at(59)), "287082");
        assert_eq!(code_for_step(RFC_SECRET, step_at(1111111109)), "081804");
    }

    #[test]
    fn rfc4226_hotp_vectors() {
        let want = [
            755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
        ];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(hotp(RFC_SECRET, i as u64, 6), *w);
        }
    }

    #[test]
    fn window_and_replay() {
        let now = 1_700_000_000u64;
        let step = step_at(now);
        let code_now = code_for_step(RFC_SECRET, step);
        let code_prev = code_for_step(RFC_SECRET, step - 1);
        let code_next = code_for_step(RFC_SECRET, step + 1);
        let code_old = code_for_step(RFC_SECRET, step - 2);
        assert_eq!(check_code(RFC_SECRET, &code_now, now, 0), Some(step));
        assert_eq!(check_code(RFC_SECRET, &code_prev, now, 0), Some(step - 1));
        assert_eq!(check_code(RFC_SECRET, &code_next, now, 0), Some(step + 1));
        assert_eq!(check_code(RFC_SECRET, &code_old, now, 0), None);
        // Used once: the same step (or an older one) is refused.
        assert_eq!(check_code(RFC_SECRET, &code_now, now, step), None);
        assert_eq!(check_code(RFC_SECRET, &code_prev, now, step), None);
        assert_eq!(
            check_code(RFC_SECRET, &code_next, now, step),
            Some(step + 1)
        );
        // Spaces are fine; anything else isn't.
        let spaced = format!("{} {}", &code_now[..3], &code_now[3..]);
        assert_eq!(check_code(RFC_SECRET, &spaced, now, 0), Some(step));
        assert_eq!(check_code(RFC_SECRET, "12345", now, 0), None);
        assert_eq!(check_code(RFC_SECRET, "abcdef", now, 0), None);
    }

    #[test]
    fn base32_round_trip() {
        assert_eq!(
            base32_encode(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        let s = new_secret();
        assert_eq!(s.len(), 20);
        assert_eq!(base32_decode(&base32_encode(&s)).unwrap(), s);
        assert_eq!(base32_decode("mzxw 6ytb oi").unwrap(), b"foobar");
        assert!(base32_decode("not base32!").is_none());
    }

    #[test]
    fn otpauth_link() {
        let uri = otpauth_uri("ann.b", "ABC");
        assert_eq!(
            uri,
            "otpauth://totp/Outhouse:ann.b?secret=ABC&issuer=Outhouse&algorithm=SHA1&digits=6&period=30"
        );
        let svg = qr_svg(&uri).unwrap();
        assert!(svg.contains("<svg") && svg.contains("#000000"));
    }

    #[test]
    fn recovery_codes_are_distinct_and_normalised() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), 10);
        for c in &codes {
            assert_eq!(c.len(), 11);
            assert!(looks_like_recovery(c));
            assert_eq!(
                recovery_hash(c),
                recovery_hash(&c.to_uppercase().replace('-', " "))
            );
        }
        assert!(!looks_like_recovery("123456"));
        assert_ne!(recovery_hash(&codes[0]), recovery_hash(&codes[1]));
    }

    #[test]
    fn secrets_round_trip_and_are_bound_to_the_account() {
        let key = derive(b"0123456789abcdef0123456789abcdef");
        let other = derive(b"another-key-another-key-another-key");
        let secret = new_secret();
        let sealed = encrypt_with(&key, 42, &secret).unwrap();
        assert!(sealed.starts_with("v1."));
        assert_eq!(decrypt_with(&key, 42, &sealed).unwrap(), secret);
        // Another account's row, another key, or a changed byte: nothing.
        assert!(decrypt_with(&key, 43, &sealed).is_none());
        assert!(decrypt_with(&other, 42, &sealed).is_none());
        let mut bytes = sealed.into_bytes();
        let last = bytes.len() - 2;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        assert!(decrypt_with(&key, 42, &String::from_utf8(bytes).unwrap()).is_none());
        // Two seals of the same secret differ (fresh nonce).
        assert_ne!(
            encrypt_with(&key, 42, &secret).unwrap(),
            encrypt_with(&key, 42, &secret).unwrap()
        );
    }

    #[test]
    fn key_file_is_made_once() {
        let dir = std::env::temp_dir().join(format!("oh-key-{}", now_unix() ^ 0x5a5a));
        let a = file_key(&dir).unwrap();
        let b = file_key(&dir).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
