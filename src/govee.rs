//! Govee LED strips over the official LAN API: JSON datagrams sent to UDP port
//! 4003 on the strip. "LAN Control" must be enabled for the device in the Govee
//! Home app. Control is fire-and-forget; only an explicit scan reads replies.

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::color::Rgb;

const MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SCAN_PORT: u16 = 4001;
const REPLY_PORT: u16 = 4002;
const CONTROL_PORT: u16 = 4003;

/// The strip's controller drops commands sent faster than this.
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// UDP may lose a datagram: an unchanged color is resent this often.
pub const REFRESH: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug)]
pub struct GoveeConfig {
    pub ip: Ipv4Addr,
    /// Strip brightness set once on connect, 1-100; `None` keeps the current one.
    pub brightness: Option<u8>,
}

fn command(cmd: &str, data: Value) -> String {
    json!({ "msg": { "cmd": cmd, "data": data } }).to_string()
}

pub fn turn(on: bool) -> String {
    command("turn", json!({ "value": u8::from(on) }))
}

pub fn brightness(percent: u8) -> String {
    command("brightness", json!({ "value": percent }))
}

pub fn color(c: Rgb) -> String {
    command("colorwc", json!({ "color": { "r": c.r, "g": c.g, "b": c.b }, "colorTemInKelvin": 0 }))
}

/// Send-only connection to one strip.
pub struct Strip {
    socket: UdpSocket,
}

impl Strip {
    /// Switches the strip on and applies the configured brightness.
    pub fn open(config: GoveeConfig) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.connect((config.ip, CONTROL_PORT))?;
        let strip = Strip { socket };
        strip.send(&turn(true))?;
        if let Some(b) = config.brightness {
            strip.send(&brightness(b))?;
        }
        Ok(strip)
    }

    pub fn show(&self, c: Rgb) -> io::Result<()> {
        self.send(&color(c))
    }

    fn send(&self, message: &str) -> io::Result<()> {
        self.socket.send(message.as_bytes()).map(drop)
    }
}

#[derive(Debug)]
pub struct Device {
    pub ip: Ipv4Addr,
    pub sku: String,
    pub id: String,
}

/// Asks every strip on the LAN to announce itself and collects the replies
/// for `timeout`. Many routers drop multicast towards Wi-Fi clients, so the
/// request also goes to the broadcast address of each local /24 network.
pub fn scan(timeout: Duration) -> io::Result<Vec<Device>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, REPLY_PORT))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    socket.set_broadcast(true)?;
    let request = command("scan", json!({ "account_topic": "reserve" }));
    socket.send_to(request.as_bytes(), (MULTICAST, SCAN_PORT))?;
    for ip in local_networks() {
        let [a, b, c, _] = ip.octets();
        // A network that cannot route the broadcast is simply skipped.
        let _ = socket.send_to(request.as_bytes(), (Ipv4Addr::new(a, b, c, 255), SCAN_PORT));
    }

    let deadline = Instant::now() + timeout;
    let mut found = BTreeMap::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                if let Some(device) = parse_scan_reply(&buf[..n]) {
                    found.insert(device.ip, device);
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(found.into_values().collect())
}

/// Private IPv4 addresses of this PC, from resolving its own host name.
fn local_networks() -> Vec<Ipv4Addr> {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());
    let mut ips: Vec<Ipv4Addr> = (host.as_str(), 0)
        .to_socket_addrs()
        .map(|addrs| {
            addrs
                .filter_map(|a| match a.ip() {
                    IpAddr::V4(ip) if ip.is_private() => Some(ip),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    ips.sort();
    ips.dedup();
    ips
}

fn parse_scan_reply(bytes: &[u8]) -> Option<Device> {
    let reply: Value = serde_json::from_slice(bytes).ok()?;
    let data = reply.get("msg").filter(|m| m["cmd"] == "scan")?.get("data")?;
    let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    Some(Device { ip: text("ip").parse().ok()?, sku: text("sku"), id: text("device") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_lan_api() {
        let parsed: Value = serde_json::from_str(&color(Rgb { r: 255, g: 10, b: 0 })).unwrap();
        assert_eq!(
            parsed,
            json!({"msg": {"cmd": "colorwc", "data": {"color": {"r": 255, "g": 10, "b": 0}, "colorTemInKelvin": 0}}})
        );
        assert_eq!(turn(true), r#"{"msg":{"cmd":"turn","data":{"value":1}}}"#);
        assert_eq!(brightness(40), r#"{"msg":{"cmd":"brightness","data":{"value":40}}}"#);
    }

    #[test]
    fn parses_scan_reply() {
        let reply = br#"{"msg":{"cmd":"scan","data":{"ip":"192.168.1.23","device":"1F:80:C5:32:32:36:72:4E","sku":"H619A","bleVersionHard":"3.01.01","wifiVersionSoft":"1.00.10"}}}"#;
        let device = parse_scan_reply(reply).unwrap();
        assert_eq!(device.ip, Ipv4Addr::new(192, 168, 1, 23));
        assert_eq!(device.sku, "H619A");
        assert!(parse_scan_reply(br#"{"msg":{"cmd":"devStatus","data":{}}}"#).is_none());
    }
}
