//! Minimal miIO LAN protocol client (UDP 54321).
//!
//! Packet: 32-byte header + AES-128-CBC encrypted JSON-RPC payload.
//!   header   = magic 0x2131 | length | 0 | device id | stamp | md5 checksum
//!   key      = md5(token), iv = md5(key + token)
//!   checksum = md5(header with the token in the checksum field + payload)
//!
//! A "hello" (a header of 0xff bytes) makes the device answer with its id and
//! current stamp; every request must carry a stamp ahead of the device's.

use std::fmt;
use std::io::ErrorKind;
use std::net::UdpSocket;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::{Context, Result, anyhow, bail};
use md5::{Digest, Md5};
use serde_json::{Value, json};

type Encryptor = cbc::Encryptor<aes::Aes128>;
type Decryptor = cbc::Decryptor<aes::Aes128>;

pub const PORT: u16 = 54321;

const HELLO: [u8; 32] = {
    let mut packet = [0xff; 32];
    packet[0] = 0x21;
    packet[1] = 0x31;
    packet[2] = 0x00;
    packet[3] = 0x20;
    packet
};

/// The device did not answer at all; distinct from a wrong token or a rejected
/// request so callers can go looking for it at a new address.
#[derive(Debug)]
pub struct NoReply(pub String);

impl fmt::Display for NoReply {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NoReply {}

fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut hasher = Md5::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_token(hex: &str) -> Result<[u8; 16]> {
    let hex = hex.trim();
    if hex.len() != 32 {
        bail!("token must be 32 hex characters");
    }
    let mut token = [0u8; 16];
    for (i, byte) in token.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).context("token is not hex")?;
    }
    Ok(token)
}

pub struct Device {
    pub host: String,
    token: [u8; 16],
    key: [u8; 16],
    iv: [u8; 16],
    device_id: u32,
    stamp_offset: i64,
    next_id: u32,
    timeout: Duration,
    retries: u32,
}

impl Device {
    pub fn new(host: &str, token: &str) -> Result<Self> {
        let token = parse_token(token)?;
        let key = md5(&[&token]);
        let iv = md5(&[&key, &token]);
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        Ok(Self {
            host: host.to_string(),
            token,
            key,
            iv,
            device_id: 0,
            stamp_offset: 0,
            next_id: seed % 10_000 + 1,
            timeout: Duration::from_secs(3),
            retries: 3,
        })
    }

    fn exchange(&self, packet: &[u8]) -> std::io::Result<Vec<u8>> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_read_timeout(Some(self.timeout))?;
        socket.send_to(packet, (self.host.as_str(), PORT))?;
        let mut buffer = [0u8; 4096];
        let (length, _) = socket.recv_from(&mut buffer)?;
        Ok(buffer[..length].to_vec())
    }

    fn handshake(&mut self) -> std::io::Result<()> {
        let reply = self.exchange(&HELLO)?;
        if reply.len() < 16 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "short hello reply",
            ));
        }
        self.device_id = u32::from_be_bytes(reply[8..12].try_into().unwrap());
        let stamp = u32::from_be_bytes(reply[12..16].try_into().unwrap());
        self.stamp_offset = stamp as i64 - unix_now();
        Ok(())
    }

    fn packet(&self, payload: &[u8]) -> Vec<u8> {
        let mut plain = payload.to_vec();
        plain.push(0);
        let body = Encryptor::new(&self.key.into(), &self.iv.into())
            .encrypt_padded_vec_mut::<Pkcs7>(&plain);
        let stamp = (unix_now() + self.stamp_offset + 1) as u32;

        let mut header = Vec::with_capacity(16);
        header.extend_from_slice(&0x2131u16.to_be_bytes());
        header.extend_from_slice(&((32 + body.len()) as u16).to_be_bytes());
        header.extend_from_slice(&0u32.to_be_bytes());
        header.extend_from_slice(&self.device_id.to_be_bytes());
        header.extend_from_slice(&stamp.to_be_bytes());
        let checksum = md5(&[&header, &self.token, &body]);

        [header, checksum.to_vec(), body].concat()
    }

    fn decrypt(&self, data: &[u8]) -> Result<Value> {
        let plain = Decryptor::new(&self.key.into(), &self.iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .map_err(|_| anyhow!("could not decrypt the reply: the token is wrong"))?;
        let end = plain.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        Ok(serde_json::from_slice(&plain[..end])?)
    }

    pub fn send(&mut self, method: &str, params: Value) -> Result<Value> {
        let mut last_error = String::from("no attempt made");
        for attempt in 0..self.retries {
            if (self.device_id == 0 || attempt > 0)
                && let Err(e) = self.handshake()
            {
                last_error = e.to_string();
                continue;
            }
            self.next_id += 1;
            let id = self.next_id;
            let request = json!({ "id": id, "method": method, "params": params });
            let reply = match self.exchange(&self.packet(request.to_string().as_bytes())) {
                Ok(reply) => reply,
                Err(e) => {
                    last_error = e.to_string();
                    continue;
                }
            };
            if reply.len() <= 32 {
                last_error = "empty reply (wrong token?)".into();
                continue;
            }
            let result = self.decrypt(&reply[32..])?;
            if result["id"] != json!(id) {
                last_error = "reply id mismatch".into();
                continue;
            }
            if let Some(error) = result.get("error") {
                bail!("{method}: {error}");
            }
            return Ok(result.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(NoReply(format!("no reply from {}: {last_error}", self.host)).into())
    }

    /// The device id from the last handshake (0 before the first request).
    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    /// `miIO.info`: model, firmware, MAC, Wi-Fi details.
    pub fn info(&mut self) -> Result<Value> {
        self.send("miIO.info", json!([]))
    }

    /// MIoT get_properties; returns the per-property reply objects ({code, value}).
    pub fn get_properties(&mut self, props: &[(u32, u32)]) -> Result<Vec<Value>> {
        let params: Vec<Value> = props
            .iter()
            .map(|(s, p)| json!({ "did": format!("{s}-{p}"), "siid": s, "piid": p }))
            .collect();
        replies(self.send("get_properties", Value::Array(params))?)
    }

    pub fn set_properties(&mut self, props: &[(u32, u32, Value)]) -> Result<Vec<Value>> {
        let params: Vec<Value> = props
            .iter()
            .map(|(s, p, v)| json!({ "did": format!("{s}-{p}"), "siid": s, "piid": p, "value": v }))
            .collect();
        replies(self.send("set_properties", Value::Array(params))?)
    }
}

fn replies(result: Value) -> Result<Vec<Value>> {
    match result {
        Value::Array(items) => Ok(items),
        other => bail!("unexpected reply: {other}"),
    }
}

/// Find a device's current IP by sending hellos across `near_host`'s /24.
/// The AC ignores broadcast hellos, so every address is probed directly.
pub fn discover(device_id: u32, near_host: &str, wait: Duration) -> Option<String> {
    let prefix = near_host.rsplit_once('.')?.0;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_nonblocking(true).ok()?;
    for last in 1..=254 {
        // Unroutable or ARP-less addresses just fail; keep going.
        let _ = socket.send_to(&HELLO, (format!("{prefix}.{last}").as_str(), PORT));
    }
    let deadline = Instant::now() + wait;
    let mut buffer = [0u8; 64];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, from)) if length >= 12 => {
                if u32::from_be_bytes(buffer[8..12].try_into().unwrap()) == device_id {
                    return Some(from.ip().to_string());
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_round_trips_through_the_token() {
        let mut device = Device::new("127.0.0.1", "00112233445566778899aabbccddeeff").unwrap();
        device.device_id = 0x1234_5678;
        let payload = br#"{"id":1,"method":"miIO.info","params":[]}"#;
        let packet = device.packet(payload);

        assert_eq!(&packet[..2], &[0x21, 0x31]);
        assert_eq!(
            u16::from_be_bytes([packet[2], packet[3]]) as usize,
            packet.len()
        );
        assert_eq!(
            u32::from_be_bytes(packet[8..12].try_into().unwrap()),
            0x1234_5678
        );

        let header_with_token = [&packet[..16], &device.token[..], &packet[32..]].concat();
        assert_eq!(&packet[16..32], &md5(&[&header_with_token]));

        let decoded = device.decrypt(&packet[32..]).unwrap();
        assert_eq!(decoded["method"], "miIO.info");
    }

    #[test]
    fn rejects_malformed_tokens() {
        assert!(Device::new("127.0.0.1", "abc").is_err());
        assert!(Device::new("127.0.0.1", "zz112233445566778899aabbccddeeff").is_err());
    }
}
