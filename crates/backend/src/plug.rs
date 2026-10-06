//! Local-network control of the Tuya smart plug the laptop charger is plugged
//! into.
//!
//! A trimmed port of the `tuya-plug` CLI: only protocol 3.5 (the `6699` frame
//! format, AES-128-GCM with the frame header as associated data), since that is
//! what this one plug speaks. The plug's identity is hard-coded below.
//!
//! Everything here is blocking std I/O with short timeouts; call it from
//! `tokio::task::spawn_blocking`.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use anyhow::{Context, Result, anyhow, bail, ensure};
use hmac::{Hmac, Mac};
use serde_json::{Map, Value, json};
use sha2::Sha256;

const DEVICE_ID: &str = "bf47379267c7fb7c82rxpa";
const ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 115)), 6668);
const LOCAL_KEY: &Key = b"`lT`.gzc~.Q8Xwdd";
/// The data point that holds the relay state.
const SWITCH_DP: &str = "1";

const TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the plug to report the new state after a switch.
const CONFIRM_TIMEOUT: Duration = Duration::from_millis(1500);

const SESS_KEY_NEG_START: u32 = 0x03;
const SESS_KEY_NEG_RESP: u32 = 0x04;
const SESS_KEY_NEG_FINISH: u32 = 0x05;
const HEART_BEAT: u32 = 0x09;
const CONTROL_NEW: u32 = 0x0d;
const DP_QUERY_NEW: u32 = 0x10;

const PREFIX: u32 = 0x0000_6699;
const SUFFIX: u32 = 0x0000_9966;
/// Sanity limit; real device payloads are a few hundred bytes.
const MAX_FRAME_LEN: usize = 0x1000;
/// `"3.5"` followed by 12 zero bytes, prepended to control commands.
const VERSION_HEADER: [u8; 15] = *b"3.5\0\0\0\0\0\0\0\0\0\0\0\0";

type Key = [u8; 16];
type Dps = Map<String, Value>;

/// Whether the plug is currently on.
pub fn is_on() -> Result<bool> {
    Session::connect()?.read_switch()
}

/// Switches the plug and checks that it reports the requested state back.
pub fn switch(on: bool) -> Result<()> {
    let mut session = Session::connect()?;
    let reported = match session.set_switch(on)? {
        Some(state) => state,
        // The plug only acknowledged the command; read the state back.
        None => session.read_switch()?,
    };
    ensure!(reported == on, "plug reported {} after switching", if reported { "ON" } else { "OFF" });
    Ok(())
}

struct Session {
    stream: TcpStream,
    /// The session key negotiated in `connect`.
    key: Key,
    seq: u32,
}

impl Session {
    fn connect() -> Result<Self> {
        let stream =
            TcpStream::connect_timeout(&ADDR, TIMEOUT).with_context(|| format!("connecting to plug {ADDR}"))?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.set_nodelay(true)?;

        let mut session = Session { stream, key: *LOCAL_KEY, seq: 1 };
        session.negotiate_session_key().context("session key negotiation with the plug failed")?;
        Ok(session)
    }

    fn read_switch(&mut self) -> Result<bool> {
        let body = json!({ "gwId": DEVICE_ID, "devId": DEVICE_ID, "uid": DEVICE_ID, "t": now().to_string() });
        self.send(DP_QUERY_NEW, body.to_string().as_bytes())?;
        let dps = self
            .wait_for_dps(Instant::now() + TIMEOUT)?
            .ok_or_else(|| anyhow!("plug did not answer the status query"))?;
        dps.get(SWITCH_DP)
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("data point {SWITCH_DP} is not a boolean switch; dps: {}", Value::from(dps.clone())))
    }

    /// Returns the state the plug reported back, or `None` if it only
    /// acknowledged the command.
    fn set_switch(&mut self, on: bool) -> Result<Option<bool>> {
        let body = json!({ "protocol": 5, "t": now(), "data": { "dps": { SWITCH_DP: on } } });
        self.send(CONTROL_NEW, body.to_string().as_bytes())?;

        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        while let Some(dps) = self.wait_for_dps(deadline)? {
            if let Some(state) = dps.get(SWITCH_DP).and_then(Value::as_bool) {
                return Ok(Some(state));
            }
        }
        Ok(None)
    }

    /// Exchanges nonces with the plug and derives a per-session AES key.
    fn negotiate_session_key(&mut self) -> Result<()> {
        let local_nonce: [u8; 16] = rand::random();
        self.send(SESS_KEY_NEG_START, &local_nonce)?;

        let data = loop {
            let frame = self.recv()?;
            if frame.cmd == SESS_KEY_NEG_RESP {
                break frame.payload;
            }
        };
        ensure!(data.len() >= 48, "short handshake response ({} bytes)", data.len());
        let (remote_nonce, mac) = (&data[..16], &data[16..48]);
        ensure!(mac == hmac_sha256(LOCAL_KEY, &local_nonce), "plug failed to authenticate (wrong local key?)");
        self.send(SESS_KEY_NEG_FINISH, &hmac_sha256(LOCAL_KEY, remote_nonce))?;

        let mut mixed = [0u8; 16];
        for (i, b) in mixed.iter_mut().enumerate() {
            *b = local_nonce[i] ^ remote_nonce[i];
        }
        let session = gcm_encrypt(LOCAL_KEY, &local_nonce[..12], &mixed, &[])?;
        self.key.copy_from_slice(&session[..16]);
        Ok(())
    }

    fn send(&mut self, cmd: u32, payload: &[u8]) -> Result<()> {
        let mut plain = Vec::with_capacity(VERSION_HEADER.len() + payload.len());
        if cmd == CONTROL_NEW {
            plain.extend_from_slice(&VERSION_HEADER);
        }
        plain.extend_from_slice(payload);
        let frame = pack_frame(self.seq, cmd, &plain, &self.key, &rand::random())?;
        self.seq += 1;
        self.stream.write_all(&frame).context("sending to plug")?;
        Ok(())
    }

    fn recv(&mut self) -> Result<Frame> {
        read_frame(&mut self.stream, &self.key).map_err(|e| {
            // The plug silently drops the connection on a bad key.
            match e.downcast_ref::<io::Error>() {
                Some(io_err) if io_err.kind() == io::ErrorKind::UnexpectedEof => anyhow!("plug closed the connection"),
                _ => e,
            }
        })
    }

    /// Reads frames until one carries data points. Returns `None` on timeout.
    fn wait_for_dps(&mut self, deadline: Instant) -> Result<Option<Dps>> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(remaining))?;
            let frame = match self.recv() {
                Ok(frame) => frame,
                Err(e) if is_timeout(&e) => return Ok(None),
                Err(e) => return Err(e),
            };
            let data = strip_version_header(&frame.payload);
            if frame.cmd == HEART_BEAT || data.is_empty() {
                continue;
            }

            let Ok(msg) = serde_json::from_slice::<Value>(data) else {
                bail!(
                    "plug replied with an error (code {}): {}",
                    frame.retcode.unwrap_or_default(),
                    String::from_utf8_lossy(data)
                );
            };
            if let Some(Value::Object(dps)) = msg.get("dps").or_else(|| msg.pointer("/data/dps")) {
                return Ok(Some(dps.clone()));
            }
        }
    }
}

#[derive(Debug)]
struct Frame {
    cmd: u32,
    retcode: Option<u32>,
    /// Decrypted payload, with the return code stripped.
    payload: Vec<u8>,
}

fn pack_frame(seq: u32, cmd: u32, plaintext: &[u8], key: &Key, iv: &[u8; 12]) -> Result<Vec<u8>> {
    let len = 12 + plaintext.len() + 16;
    let mut buf = Vec::with_capacity(18 + len + 4);
    buf.extend(PREFIX.to_be_bytes());
    buf.extend(0u16.to_be_bytes());
    buf.extend(seq.to_be_bytes());
    buf.extend(cmd.to_be_bytes());
    buf.extend((len as u32).to_be_bytes());
    let sealed = gcm_encrypt(key, iv, plaintext, &buf[4..])?;
    buf.extend_from_slice(iv);
    buf.extend(sealed);
    buf.extend(SUFFIX.to_be_bytes());
    Ok(buf)
}

fn read_frame(r: &mut impl Read, key: &Key) -> Result<Frame> {
    let mut header = [0u8; 18];
    r.read_exact(&mut header)?;
    let prefix = be_u32(&header[..4]);
    ensure!(prefix == PREFIX, "unexpected frame prefix {prefix:#010x}");
    let cmd = be_u32(&header[10..14]);
    let len = be_u32(&header[14..18]) as usize;
    ensure!((28..MAX_FRAME_LEN).contains(&len), "bad frame length {len}");

    let mut body = vec![0u8; len + 4];
    r.read_exact(&mut body)?;
    ensure!(be_u32(&body[len..]) == SUFFIX, "bad frame suffix");
    let (iv, sealed) = body[..len].split_at(12);
    let plaintext = gcm_decrypt(key, iv, sealed, &header[4..])?;

    let (retcode, payload) = split_retcode(&plaintext);
    Ok(Frame { cmd, retcode, payload: payload.to_vec() })
}

/// Device→client frames carry a 4-byte return code before the payload. Like
/// other Tuya clients, detect it by its small value (top 3 bytes zero).
fn split_retcode(payload: &[u8]) -> (Option<u32>, &[u8]) {
    if payload.len() >= 4 && payload[..3] == [0, 0, 0] {
        (Some(payload[3] as u32), &payload[4..])
    } else {
        (None, payload)
    }
}

fn strip_version_header(payload: &[u8]) -> &[u8] {
    if payload.len() >= 15 && payload.starts_with(b"3.") { &payload[15..] } else { payload }
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Returns `ciphertext || tag`.
fn gcm_encrypt(key: &Key, iv: &[u8], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    Aes128Gcm::new(key.into())
        .encrypt(iv.into(), Payload { msg: plaintext, aad })
        .map_err(|_| anyhow!("AES-GCM encryption failed"))
}

fn gcm_decrypt(key: &Key, iv: &[u8], sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    Aes128Gcm::new(key.into())
        .decrypt(iv.into(), Payload { msg: sealed, aad })
        .map_err(|_| anyhow!("AES-GCM decryption failed (wrong local key?)"))
}

fn is_timeout(e: &anyhow::Error) -> bool {
    e.downcast_ref::<io::Error>()
        .is_some_and(|e| matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &Key = b"0123456789abcdef";

    #[test]
    fn frame_roundtrip() {
        let frame = pack_frame(7, DP_QUERY_NEW, b"\0\0\0\0{}", KEY, &[1; 12]).unwrap();
        let parsed = read_frame(&mut &frame[..], KEY).unwrap();
        assert_eq!((parsed.cmd, parsed.retcode), (DP_QUERY_NEW, Some(0)));
        assert_eq!(parsed.payload, b"{}");
    }

    #[test]
    fn frame_rejects_wrong_key() {
        let frame = pack_frame(7, DP_QUERY_NEW, b"{}", KEY, &[1; 12]).unwrap();
        assert!(read_frame(&mut &frame[..], b"fedcba9876543210").is_err());
    }

    /// Talks to the real plug, read-only: `cargo test -p backend -- --ignored`.
    #[test]
    #[ignore = "needs the plug on the local network"]
    fn reads_the_real_plug() {
        println!("plug is {}", if is_on().unwrap() { "ON" } else { "OFF" });
    }
}
