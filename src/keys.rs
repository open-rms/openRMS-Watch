//! openRMS API keys (`ormk_` + a JWT minted by openRMS-Accounts, Spec accounts/identity.md §5).
//!
//! The watcher holds one renewable Atlas key with scope `watch:ingest`. It never verifies the
//! signature (Atlas does); it only reads the claims to know when to renew. Renewal
//! (`POST {issuer}/api/v1/keys/renew`) returns a key with `replaces`, which Atlas binds to the
//! old key's watcher on first use. Linking (`POST {atlas}/api/v1/watch/link`) binds a key to a
//! watcher once.

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

pub const PREFIX: &str = "ormk_";
/// What the watcher asks Accounts for (its client policy allows exactly this).
pub const SCOPE: &str = "watch:ingest";
pub const MAX_DAYS: u32 = 365;
/// The key's name on the person's API keys page at Accounts.
pub const KEY_NAME: &str = "openRMS Watch";

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Claims {
    #[serde(default)]
    pub iss: String,
    #[serde(default)]
    #[allow(dead_code)] // for logs and tests
    pub jti: String,
    #[serde(default)]
    pub iat: i64,
    #[serde(default)]
    pub exp: i64,
    #[serde(default)]
    pub renewable: bool,
}

/// The claims of an `ormk_` key, unverified. None if it isn't one.
pub fn claims(key: &str) -> Option<Claims> {
    let jwt = key.trim().strip_prefix(PREFIX)?;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Renew when less than a third of the lifetime is left, and never later than 30 days
/// before expiry (a year-long key renews a month ahead; Accounts mails the owner at 14 days).
pub fn renew_due(c: &Claims, now_s: i64) -> bool {
    if !c.renewable || c.exp <= 0 {
        return false;
    }
    let life = (c.exp - c.iat).max(0);
    let ahead = (life / 3).min(30 * 86400);
    now_s >= c.exp - ahead
}

pub fn expired(c: &Claims, now_s: i64) -> bool {
    c.exp > 0 && now_s >= c.exp
}

fn trim(u: &str) -> &str {
    u.trim().trim_end_matches('/')
}

/// Whether a key was issued by this Accounts (so renewal goes to the right place).
pub fn issued_by(c: &Claims, accounts: &str) -> bool {
    trim(&c.iss) == trim(accounts)
}

fn http_err(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(String::from))
                .unwrap_or(body);
            anyhow::anyhow!("HTTP {code}: {msg}")
        }
        e => e.into(),
    }
}

/// Self-renewal: the current key buys its replacement (the old one is then revoked).
pub fn renew(agent: &ureq::Agent, accounts: &str, key: &str) -> Result<String> {
    let v: Value = agent
        .post(&format!("{}/api/v1/keys/renew", trim(accounts)))
        .set("authorization", &format!("Bearer {key}"))
        .call()
        .map_err(http_err)?
        .into_json()
        .context("renewal response")?;
    match v["key"].as_str() {
        Some(k) if k.starts_with(PREFIX) => Ok(k.to_string()),
        _ => bail!("no key in Accounts' answer"),
    }
}

/// Mint the watcher's Atlas key with a device-flow access token for Accounts (scope `keys`).
/// Err carries the HTTP status so the caller can refresh on 401.
pub fn mint(issuer: &str, access_token: &str, atlas: &str) -> std::result::Result<String, (u16, String)> {
    let r = ureq::post(&format!("{}/api/v1/keys", trim(issuer)))
        .set("authorization", &format!("Bearer {access_token}"))
        .send_json(json!({"aud": trim(atlas), "scopes": [SCOPE], "name": KEY_NAME, "expires_in_days": MAX_DAYS}));
    match r {
        Ok(r) => {
            let v: Value = r.into_json().map_err(|e| (0, e.to_string()))?;
            v["key"].as_str().filter(|k| k.starts_with(PREFIX)).map(String::from).ok_or((0, "no key in Accounts' answer".into()))
        }
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or(body);
            Err((code, msg))
        }
        Err(e) => Err((0, e.to_string())),
    }
}

/// What to bind a key to at Atlas.
pub enum Target<'a> {
    Watcher(&'a str),
    New { name: &'a str, gridsquare: Option<&'a str> },
}

#[derive(Debug, Clone)]
pub struct Linked {
    pub watcher_id: String,
    pub name: String,
}

/// Bind `key` to a watcher at Atlas (relinking moves it). Returns the watcher.
pub fn link(agent: &ureq::Agent, atlas: &str, key: &str, target: Target) -> Result<Linked> {
    let body = match target {
        Target::Watcher(id) => json!({"watcherId": id}),
        Target::New { name, gridsquare } => {
            let mut b = json!({"name": name});
            if let Some(g) = gridsquare.map(str::trim).filter(|g| !g.is_empty()) {
                b["gridsquare"] = g.into();
            }
            b
        }
    };
    let v: Value = agent
        .post(&format!("{}/api/v1/watch/link", trim(atlas)))
        .set("authorization", &format!("Bearer {key}"))
        .send_json(body)
        .map_err(http_err)?
        .into_json()
        .context("link response")?;
    let watcher_id = v["watcherId"].as_str().context("no watcherId in Atlas's answer")?.to_string();
    Ok(Linked { watcher_id, name: v["name"].as_str().unwrap_or("watcher").to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(payload: &str) -> String {
        let enc = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        format!("{PREFIX}{}.{}.c2ln", enc(r#"{"typ":"openrms-key+jwt","alg":"EdDSA"}"#), enc(payload))
    }

    #[test]
    fn reads_claims_and_schedules_renewal() {
        let day = 86400;
        let k = key(&format!(r#"{{"iss":"https://auth.openrms.dev","jti":"j1","iat":0,"exp":{},"renewable":true,"scope":"watch:ingest"}}"#, 365 * day));
        let c = claims(&k).unwrap();
        assert_eq!(c.jti, "j1");
        assert!(issued_by(&c, "https://auth.openrms.dev/"));
        assert!(!issued_by(&c, "http://localhost:8789"));
        assert!(!renew_due(&c, 334 * day));
        assert!(renew_due(&c, 335 * day)); // 30 days ahead
        // a 30-day key renews with 10 days left
        let c30 = claims(&key(&format!(r#"{{"iat":0,"exp":{},"renewable":true}}"#, 30 * day))).unwrap();
        assert!(!renew_due(&c30, 19 * day) && renew_due(&c30, 20 * day));
        // keys made on the keys page can't renew themselves
        let fixed = claims(&key(&format!(r#"{{"iat":0,"exp":{}}}"#, 90 * day))).unwrap();
        assert!(!renew_due(&fixed, 89 * day) && expired(&fixed, 90 * day));
        assert!(claims("ormsw_abc").is_none() && claims("ormk_nope").is_none());
    }
}
