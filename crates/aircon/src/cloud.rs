//! Xiaomi cloud: QR-code login and device listing, to fetch the AC's local token.
//!
//! Newer firmware no longer reveals the miIO token in the LAN handshake; only
//! the AC and Xiaomi's cloud know it. `aircon setup` uses this once, then all
//! control is local. The cloud session is kept in memory only, never saved.
//!
//! The protocol is the one the Mi Home app speaks, as documented by
//! PiotrMachowski/Xiaomi-cloud-tokens-extractor:
//!   1. GET account.xiaomi.com/longPolling/loginUrl -> QR image + long-poll URL
//!   2. long-poll until the QR is scanned in Mi Home -> ssecurity, userId, location
//!   3. follow `location` -> serviceToken cookie
//!   4. API calls to <region>.api.io.mi.com are RC4-encrypted with a key derived
//!      from ssecurity and a per-request nonce.

use std::io::Read;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};

const REGIONS: &[&str] = &["sg", "cn", "de", "us", "i2", "ru", "tw", "in"];

pub struct QrLogin {
    pub image_url: String,
    pub login_url: String,
    poll_url: String,
    timeout: Duration,
}

#[derive(Default)]
struct Session {
    user_id: String,
    ssecurity: String,
    service_token: String,
}

pub struct Cloud {
    http: ureq::Agent,
    user_agent: String,
    session: Session,
}

fn random_bytes(count: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; count];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .expect("/dev/urandom is readable");
    bytes
}

fn random_text(alphabet: &[u8], length: usize) -> String {
    random_bytes(length)
        .iter()
        .map(|b| alphabet[*b as usize % alphabet.len()] as char)
        .collect()
}

/// Xiaomi's RC4 variant: standard RC4 with the first 1024 keystream bytes dropped.
fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + s[i] as usize + key[i % key.len()] as usize) % 256;
        s.swap(i, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut next = || {
        i = (i + 1) % 256;
        j = (j + s[i] as usize) % 256;
        s.swap(i, j);
        s[(s[i] as usize + s[j] as usize) % 256]
    };
    for _ in 0..1024 {
        next();
    }
    data.iter().map(|byte| byte ^ next()).collect()
}

/// Xiaomi prefixes some JSON answers with "&&&START&&&".
fn parse_json(text: &str) -> Result<Value> {
    Ok(serde_json::from_str(
        text.trim_start_matches("&&&START&&&"),
    )?)
}

fn read_body(response: ureq::http::Response<ureq::Body>) -> Result<String> {
    Ok(response.into_body().read_to_string()?)
}

impl Cloud {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(40)))
            .build();
        let user_agent = format!(
            "{}-{} APP/com.xiaomi.mihome APPV/10.5.201",
            random_text(b"abcdefghijklmnopqrstuvwxyz", 18),
            random_text(b"ABCDE", 13)
        );
        Self {
            http: ureq::Agent::new_with_config(config),
            user_agent,
            session: Session::default(),
        }
    }

    pub fn start_qr_login(&self) -> Result<QrLogin> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .to_string();
        let response = self
            .http
            .get("https://account.xiaomi.com/longPolling/loginUrl")
            .query_pairs([
                ("_qrsize", "480"),
                ("qs", "%3Fsid%3Dxiaomiio%26_json%3Dtrue"),
                ("callback", "https://sts.api.io.mi.com/sts"),
                ("_hasLogo", "false"),
                ("sid", "xiaomiio"),
                ("serviceParam", ""),
                ("_locale", "en_GB"),
                ("_dc", millis.as_str()),
            ])
            .call()?;
        let data = parse_json(&read_body(response)?)?;
        let text = |key: &str| {
            data[key]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("login response has no {key}: {data}"))
        };
        Ok(QrLogin {
            image_url: text("qr")?,
            login_url: text("loginUrl")?,
            poll_url: text("lp")?,
            timeout: Duration::from_secs(data["timeout"].as_u64().unwrap_or(300)),
        })
    }

    pub fn download(&self, url: &str) -> Result<Vec<u8>> {
        let mut body = self.http.get(url).call()?.into_body();
        Ok(body.read_to_vec()?)
    }

    pub fn wait_for_scan(&mut self, login: &QrLogin) -> Result<()> {
        let deadline = Instant::now() + login.timeout;
        let data = loop {
            if Instant::now() > deadline {
                bail!("the QR code was not scanned in time");
            }
            // The long poll holds the request open until the scan; a timeout just means "not yet".
            match self.http.get(&login.poll_url).call() {
                Ok(response) if response.status() == 200 => {
                    break parse_json(&read_body(response)?)?;
                }
                Ok(_) | Err(ureq::Error::Timeout(_)) => continue,
                Err(e) => return Err(e.into()),
            }
        };
        self.session.user_id = data["userId"].to_string().trim_matches('"').to_string();
        self.session.ssecurity = data["ssecurity"]
            .as_str()
            .context("no ssecurity in login reply")?
            .to_string();
        let location = data["location"]
            .as_str()
            .context("no location in login reply")?;
        self.session.service_token = self.follow_for_service_token(location)?;
        Ok(())
    }

    /// The serviceToken cookie is set somewhere along the `location` redirect chain.
    fn follow_for_service_token(&self, location: &str) -> Result<String> {
        let mut url = location.to_string();
        for _ in 0..6 {
            let response = self.http.get(&url).call()?;
            for cookie in response.headers().get_all("set-cookie") {
                let cookie = cookie.to_str().unwrap_or_default();
                if let Some(rest) = cookie.strip_prefix("serviceToken=") {
                    return Ok(rest.split(';').next().unwrap_or_default().to_string());
                }
            }
            match response
                .headers()
                .get("location")
                .and_then(|l| l.to_str().ok())
            {
                Some(next) => url = next.to_string(),
                None => break,
            }
        }
        bail!("login succeeded but no serviceToken cookie was issued")
    }

    fn signed_nonce(&self, nonce: &str) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(BASE64.decode(&self.session.ssecurity)?);
        hasher.update(BASE64.decode(nonce)?);
        Ok(BASE64.encode(hasher.finalize()))
    }

    fn signature(path: &str, params: &[(String, String)], signed_nonce: &str) -> String {
        let mut parts = vec!["POST".to_string(), path.to_string()];
        parts.extend(params.iter().map(|(k, v)| format!("{k}={v}")));
        parts.push(signed_nonce.to_string());
        BASE64.encode(Sha1::digest(parts.join("&").as_bytes()))
    }

    /// POST to https://<region>.api.io.mi.com/app<path> with the app's RC4 encryption.
    fn call(&self, path: &str, data: &Value, region: &str) -> Result<Value> {
        let host = if region == "cn" {
            "api.io.mi.com".to_string()
        } else {
            format!("{region}.api.io.mi.com")
        };
        let minutes = (SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() / 60_000) as u32;
        let nonce = BASE64.encode([random_bytes(8), minutes.to_be_bytes().to_vec()].concat());
        let signed_nonce = self.signed_nonce(&nonce)?;
        let key = BASE64.decode(&signed_nonce)?;

        let mut params = vec![("data".to_string(), data.to_string())];
        params.push((
            "rc4_hash__".into(),
            Self::signature(path, &params, &signed_nonce),
        ));
        let mut params: Vec<(String, String)> = params
            .into_iter()
            .map(|(k, v)| (k, BASE64.encode(rc4(&key, v.as_bytes()))))
            .collect();
        params.push((
            "signature".into(),
            Self::signature(path, &params, &signed_nonce),
        ));
        params.push(("ssecurity".into(), self.session.ssecurity.clone()));
        params.push(("_nonce".into(), nonce));

        let token = &self.session.service_token;
        let response = self
            .http
            .post(&format!("https://{host}/app{path}"))
            .query_pairs(params.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .header("User-Agent", &self.user_agent)
            .header("Accept-Encoding", "identity")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("x-xiaomi-protocal-flag-cli", "PROTOCAL-HTTP2")
            .header("MIOT-ENCRYPT-ALGORITHM", "ENCRYPT-RC4")
            .header(
                "Cookie",
                &format!(
                    "userId={}; yetAnotherServiceToken={token}; serviceToken={token}; locale=en_GB; channel=MI_APP_STORE",
                    self.session.user_id
                ),
            )
            .send_empty()?;
        let status = response.status();
        let body = read_body(response)?;
        if status != 200 {
            bail!("{path}: HTTP {status}");
        }
        let result: Value = serde_json::from_slice(&rc4(&key, &BASE64.decode(body.trim())?))?;
        if result["code"] != 0 {
            bail!("{path}: {result}");
        }
        Ok(result["result"].clone())
    }

    /// Every air conditioner on the account, across all regions. The same
    /// device can be listed by several regions, so results are keyed by did.
    pub fn air_conditioners(&self) -> Vec<Value> {
        let query = json!({ "getVirtualModel": true, "getHuamiDevices": 1 });
        let mut found: Vec<Value> = Vec::new();
        for region in REGIONS {
            let Ok(result) = self.call("/home/device_list", &query, region) else {
                continue;
            };
            for device in result["list"].as_array().into_iter().flatten() {
                let model = device["model"].as_str().unwrap_or_default();
                let is_ac = model.contains(".airc.") || model.contains(".aircondition.");
                if is_ac && !found.iter().any(|d| d["did"] == device["did"]) {
                    found.push(device.clone());
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_is_its_own_inverse() {
        let key = b"0123456789abcdef0123456789abcdef";
        let message = br#"{"getVirtualModel":true}"#;
        let encrypted = rc4(key, message);
        assert_ne!(encrypted, message);
        assert_eq!(rc4(key, &encrypted), message);
    }

    #[test]
    fn strips_the_start_marker() {
        assert_eq!(parse_json("&&&START&&&{\"code\":0}").unwrap()["code"], 0);
    }
}
