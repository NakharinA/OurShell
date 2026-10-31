//! Argon2id hashing for the admin page password, stored in PHC string form
//! (`$argon2id$v=19$m=...,t=...,p=...$<salt>$<hash>`) so the cost can be
//! raised later without breaking existing hashes.

use anyhow::{anyhow, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use rand::RngCore;

/// OWASP's recommended Argon2id minimum (19 MiB, 2 passes).
const M_COST_KIB: u32 = 19 * 1024;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

pub const MIN_LEN: usize = 10;

fn derive(password: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<[u8; 32]> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| anyhow!("argon2 params: {e}"))?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(out)
}

pub fn hash(password: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let out = derive(password, &salt, M_COST_KIB, T_COST, P_COST)?;
    Ok(format!(
        "$argon2id$v=19$m={M_COST_KIB},t={T_COST},p={P_COST}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(out)
    ))
}

/// `false` for a wrong password or a malformed hash.
pub fn verify(password: &str, phc: &str) -> bool {
    let parts: Vec<&str> = phc.split('$').collect();
    let ["", "argon2id", "v=19", params, salt, want] = parts.as_slice() else {
        return false;
    };
    let mut m = 0;
    let mut t = 0;
    let mut p = 0;
    for kv in params.split(',') {
        match kv.split_once('=') {
            Some(("m", v)) => m = v.parse().unwrap_or(0),
            Some(("t", v)) => t = v.parse().unwrap_or(0),
            Some(("p", v)) => p = v.parse().unwrap_or(0),
            _ => return false,
        }
    }
    let (Ok(salt), Ok(want)) = (STANDARD_NO_PAD.decode(salt), STANDARD_NO_PAD.decode(want)) else {
        return false;
    };
    let Ok(got) = derive(password, &salt, m, t, p) else {
        return false;
    };
    want.len() == got.len() && want.iter().zip(got).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$v=19$"));
        assert!(verify("correct horse battery", &h));
        assert!(!verify("wrong", &h));
        assert!(!verify("correct horse battery", "garbage"));
        assert_ne!(h, hash("correct horse battery").unwrap(), "salted");
    }
}
