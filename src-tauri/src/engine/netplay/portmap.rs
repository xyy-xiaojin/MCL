//! 端口映射直连：只在房主这台机器上开一个洞，朋友用公网地址直接连。
//!
//! 这是 LinkStar 那类工具在做的事，这里按同样的原理自己实现：
//! 1. 用 UPnP 找路由器，请它把公网端口转发到本机的游戏端口；
//! 2. 用 STUN 向公网回显服务器问一句"你看到的我是哪个地址"，
//!    拿到的 `IP:端口` 就是朋友要填的地址；
//! 3. 顺带看一眼 STUN 回的端口和本地绑定的端口是否一致——不一致说明是对称 NAT，
//!    这种网络靠映射直连基本没戏，直接劝玩家换 EasyTier 或樱花 FRP。
//!
//! 不内置 LinkStar 的 GUI 程序：它要登录自己的管理后台才能改配置，
//! 而这里只需要它那点核心能力，自己实现反而少一层进程和密码。
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use super::{publish, Channel, Fact, Slot};

/// 公网回显服务器。按"国内能通"排序，第一个不行就换下一个。
const STUN_SERVERS: [&str; 4] = [
    "stun.miwifi.com:3478",
    "stun.chat.bilibili.com:3478",
    "stun.hitv.com:3478",
    "stun.cloudflare.com:3478",
];

/// 映射想维持多久（秒）。路由器一般支持 0（永久）到 604800。
const LEASE_SECS: u32 = 7200;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapRequest {
    pub local_port: u16,
    /// 优先用 UPnP；关掉就只做 STUN 探测，告诉玩家自己去路由器开
    #[serde(default = "default_true")]
    pub use_upnp: bool,
}

fn default_true() -> bool {
    true
}

/// 探测 + 映射。整个过程是阻塞的（UDP + SSDP），所以整体丢线程池。
pub async fn start(app: AppHandle, req: MapRequest) -> crate::error::Result<Slot> {
    if req.local_port == 0 {
        return Err(crate::error::Error::msg("本地端口不能是 0"));
    }

    {
        let state = app
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| crate::error::Error::msg("应用状态未就绪"))?;
        // 与 easytier / frp 一致：把所有通道停干净再重新协商
        state.netplay.stop_all();
    }
    publish(
        &app,
        Channel::Portmap,
        Slot::starting(req.local_port, "正在和路由器协商端口…"),
    );

    let port = req.local_port;
    let use_upnp = req.use_upnp;
    let result = tokio::task::spawn_blocking(move || probe_and_map(port, use_upnp))
        .await
        .map_err(|e| crate::error::Error::msg(format!("探测线程崩了：{e}")))?;

    let slot = match result {
        Ok(mapped) => Slot {
            state: "running".to_string(),
            summary: if mapped.upnp {
                "端口已映射，朋友可以直接连".to_string()
            } else {
                "已探到公网地址，但路由器没开 UPnP".to_string()
            },
            address: format!("{}:{}", mapped.public_ip, mapped.public_port),
            local_port: req.local_port,
            error: String::new(),
            facts: mapped.facts(),
        },
        Err(e) => Slot::fail(req.local_port, e),
    };

    publish(&app, Channel::Portmap, slot.clone());
    Ok(slot)
}

/// 一次探测的结论。
#[derive(Debug, Clone)]
pub struct Mapped {
    pub public_ip: IpAddr,
    pub public_port: u16,
    pub local_ip: IpAddr,
    pub local_port: u16,
    /// UPnP 是否真的加了映射
    pub upnp: bool,
    /// STUN 回的端口和本地端口一致 → 锥形 NAT，打洞这个方向有戏
    pub cone: bool,
}

impl Mapped {
    fn facts(&self) -> Vec<Fact> {
        let mut facts = vec![
            Fact::new("内网地址", format!("{}:{}", self.local_ip, self.local_port)),
            Fact::new(
                "NAT 类型",
                if self.cone { "锥形（可直连）" } else { "对称（直连困难）" },
            ),
            Fact::new("UPnP", if self.upnp { "已开映射" } else { "未启用" }),
        ];
        if !self.upnp {
            facts.push(Fact::new(
                "还需要一步",
                format!(
                    "去路由器把 TCP {} 转发到 {}，或者改用 EasyTier",
                    self.local_port, self.local_ip
                ),
            ));
        }
        facts
    }
}

/// 阻塞的探测流程。
///
/// 两件事都不能无限等：UPnP 发现要发 SSDP 广播、STUN 要解析域名。
/// 后者在挂着代理或 DNS 抽风的机器上可能卡几十秒——`to_socket_addrs` 是同步调用，
/// 没法在外面取消，所以每个环节都套一层硬超时，超时就当"这条路不通"，
/// 让界面立刻给出结论，而不是转圈到用户以为程序死了。
fn probe_and_map(local_port: u16, use_upnp: bool) -> Result<Mapped, String> {
    let local_ip = super::lan_ip()
        .and_then(|ip| ip.parse::<IpAddr>().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

    // UPnP 和 STUN 互不依赖，并行跑
    let (upnp_tx, upnp_rx) = std::sync::mpsc::channel();
    let upnp_enabled = use_upnp;
    if use_upnp {
        std::thread::spawn(move || {
            let _ = upnp_tx.send(try_upnp(local_ip, local_port));
        });
    }

    // 有 UPnP 就不用再问 STUN：映射关系已经确定，公网地址就是网关给的那个，
    // 而且省掉一次可能卡住的域名解析。
    // 用户关掉 UPnP 时不能碰这个通道：没有发送端，recv 只能干等满超时。
    if upnp_enabled {
        if let Ok(Some((public_ip, public_port))) = upnp_rx.recv_timeout(Duration::from_secs(4)) {
            return Ok(Mapped {
                public_ip,
                public_port,
                local_ip,
                local_port,
                upnp: true,
                cone: true,
            });
        }
    }

    // 没有 UPnP：只能靠 STUN 问公网"你看到的我是谁"，用它判断直连有没有戏
    let stun = with_timeout(Duration::from_secs(7), move || stun_lookup(local_port))
        .unwrap_or_else(|| Err("STUN 探测超时：域名解析或 UDP 出网被挡了".to_string()))?;

    Ok(Mapped {
        public_ip: stun.0,
        public_port: stun.1,
        local_ip,
        local_port,
        upnp: false,
        cone: stun.1 == local_port,
    })
}

/// 把一段阻塞代码丢到独立线程，最多等 `limit`。
fn with_timeout<T, F>(limit: Duration, job: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });
    rx.recv_timeout(limit).ok()
}

/// 请路由器开一个 TCP 映射。返回公网地址 `(网关外部IP, 映射到的外部端口)`。
///
/// 失败返回 `Ok(None)`——没 UPnP 是常态，不是异常，调用方会退到 STUN 探测。
fn try_upnp(local_ip: IpAddr, local_port: u16) -> Option<(IpAddr, u16)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;

    runtime.block_on(async move {
        use igd_next::aio::tokio::search_gateway;
        use igd_next::{PortMappingProtocol, SearchOptions};

        // 默认 10 秒太长：路由器不支持 UPnP 时要干等 10 秒界面才有反应。
        // 3 秒内发现不了就当没有——这个判断对家用网络足够准。
        let options = SearchOptions {
            timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        };
        let gateway = search_gateway(options).await.ok()?;
        let external = gateway.get_external_ip().await.ok()?;
        let internal = SocketAddr::new(local_ip, local_port);

        // 先按同端口映射；被占了就往上找十个
        for offset in 0..10u16 {
            let ext_port = local_port.checked_add(offset)?;
            if gateway
                .add_port(PortMappingProtocol::TCP, ext_port, internal, LEASE_SECS, "MCL")
                .await
                .is_ok()
            {
                return Some((external, ext_port));
            }
        }
        None
    })
}

/// 向公网回显服务器要"我在外面的样子"。
fn stun_lookup(local_port: u16) -> Result<(IpAddr, u16), String> {
    // 绑在游戏端口上问：这样 NAT 记下来的映射端口和游戏端口是同一个，
    // 后面 UPnP / 手动映射才能对得上
    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), local_port))
        .or_else(|_| UdpSocket::bind("0.0.0.0:0"))
        .map_err(|e| format!("本地端口 {local_port} 绑不上：{e}"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("设置超时失败：{e}"))?;

    let request = binding_request();
    let mut last_error = String::from("STUN 服务器都没有响应");

    for server in STUN_SERVERS {
        let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&server) else {
            continue;
        };
        for addr in addrs.take(1) {
            if socket.send_to(&request, addr).is_err() {
                last_error = format!("发不出去：{server}");
                continue;
            }
            let mut buf = [0u8; 512];
            match socket.recv_from(&mut buf) {
                Ok((len, _)) => {
                    if let Some(found) = parse_mapped(&buf[..len]) {
                        return Ok(found);
                    }
                    last_error = format!("{server} 回的包里没有地址");
                }
                Err(e) => last_error = format!("{server} 没回：{e}"),
            }
        }
    }
    Err(last_error)
}

/// 20 字节的 STUN Binding Request（RFC 5389）。
fn binding_request() -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0..2].copy_from_slice(&0x0001u16.to_be_bytes()); // Binding Request
    buf[2..4].copy_from_slice(&0u16.to_be_bytes()); // 无属性
    buf[4..8].copy_from_slice(&0x2112_A442u32.to_be_bytes()); // magic cookie
    // transaction id：随便填，服务端会原样带回
    let txn: [u8; 12] = [
        0x4d, 0x43, 0x4c, 0x2d, 0x4c, 0x41, 0x4e, 0x2d, 0x30, 0x30, 0x31, 0x21,
    ];
    buf[8..20].copy_from_slice(&txn);
    buf
}

/// 解析 XOR-MAPPED-ADDRESS（0x0020），没有就退回 MAPPED-ADDRESS（0x0001）。
fn parse_mapped(packet: &[u8]) -> Option<(IpAddr, u16)> {
    if packet.len() < 20 {
        return None;
    }
    let mut pos = 20;
    while pos + 4 <= packet.len() {
        let kind = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let len = u16::from_be_bytes([packet[pos + 2], packet[pos + 3]]) as usize;
        let body_start = pos + 4;
        let body_end = body_start.checked_add(len)?;
        if body_end > packet.len() {
            return None;
        }
        let body = &packet[body_start..body_end];
        if body.len() >= 8 && (kind == 0x0020 || kind == 0x0001) {
            let port = u16::from_be_bytes([body[2], body[3]]);
            let mut octets = [body[4], body[5], body[6], body[7]];
            if kind == 0x0020 {
                // XOR 掉 magic cookie 的高 16 位和 transaction id 的前 4 字节
                let port = port ^ 0x2112;
                octets[0] ^= 0x21;
                octets[1] ^= 0x12;
                octets[2] ^= 0xA4;
                octets[3] ^= 0x42;
                return Some((IpAddr::V4(Ipv4Addr::from(octets)), port));
            }
            return Some((IpAddr::V4(Ipv4Addr::from(octets)), port));
        }
        // 属性按 4 字节对齐
        pos = body_start + ((len + 3) & !3);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_request_layout() {
        let req = binding_request();
        assert_eq!(&req[0..2], &[0x00, 0x01]);
        assert_eq!(&req[4..8], &[0x21, 0x12, 0xA4, 0x42]);
    }

    #[test]
    fn parses_xor_mapped_address() {
        // 手工拼一个带回包：XOR-MAPPED-ADDRESS 202.96.128.1:50000 → 端口 50000^0x2112
        let port: u16 = 50000;
        let octets = [202u8, 96, 128, 1];
        let xor_port = port ^ 0x2112;
        let xor_ip = [
            octets[0] ^ 0x21,
            octets[1] ^ 0x12,
            octets[2] ^ 0xA4,
            octets[3] ^ 0x42,
        ];
        let mut packet = vec![0x01, 0x01, 0x00, 0x0C, 0x21, 0x12, 0xA4, 0x42];
        packet.extend_from_slice(&[0u8; 12]);
        packet.extend_from_slice(&0x0020u16.to_be_bytes());
        packet.extend_from_slice(&8u16.to_be_bytes());
        packet.extend_from_slice(&[0x00, 0x01]);
        packet.extend_from_slice(&xor_port.to_be_bytes());
        packet.extend_from_slice(&xor_ip);

        let (ip, p) = parse_mapped(&packet).expect("应该解析出地址");
        assert_eq!(p, port);
        assert_eq!(ip.to_string(), "202.96.128.1");
    }

    #[test]
    fn ignores_truncated_packet() {
        assert!(parse_mapped(&[0, 1, 2]).is_none());
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    /// 真跑一次探测，看各阶段耗时。默认忽略，手动：
    /// `cargo test real_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_probe() {
        let start = std::time::Instant::now();
        let result = probe_and_map(25565, true);
        println!("probe took {:?}", start.elapsed());
        println!("result: {result:?}");
    }
}
