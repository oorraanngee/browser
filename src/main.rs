#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! atmo: браузер, который выходит в сеть через выбранный адаптер/адрес (Windows).
//!
//! WebView2 (Chromium) не умеет привязывать исходящие соединения к локальному IP,
//! поэтому внутри процесса поднимается прокси на 127.0.0.1. Он открывает исходящие
//! сокеты с bind() на выбранный адрес + принудительной привязкой к интерфейсу
//! (IP_UNICAST_IF / IPV6_UNICAST_IF). Любая ошибка = соединение закрыто (fail-closed).

mod game;
mod pages;
mod store;

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ipconfig::{IfType, OperStatus};
use serde_json::{json, Value};
use store::{keymap, App, Download, Shared};
use tao::dpi::LogicalSize as TaoLogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Fullscreen, Window, WindowBuilder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;
use tokio::time::timeout;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows};

// ------------------------------------------------------------------ адреса ---

#[derive(Clone)]
struct Source {
    name: String,
    label: String,
    v4: Option<Ipv4Addr>,
    v6: Option<Ipv6Addr>,
    idx4: u32,
    idx6: u32,
}

impl Source {
    fn title(&self, show_ip: bool) -> String {
        if !show_ip {
            return format!("{}  —  {}", self.label, self.name);
        }
        let mut addrs = Vec::new();
        if let Some(a) = self.v4 {
            addrs.push(a.to_string());
        }
        if let Some(a) = self.v6 {
            addrs.push(a.to_string());
        }
        format!("{}  —  {}   [{}]", addrs.join(" + "), self.label, self.name)
    }
    fn same_as(&self, o: &Source) -> bool {
        self.name == o.name && self.v4 == o.v4 && self.v6 == o.v6
    }
}

fn classify(ad: &ipconfig::Adapter, v4: Option<Ipv4Addr>) -> String {
    let n = format!("{} {}", ad.friendly_name(), ad.description()).to_lowercase();
    let has = |ks: &[&str]| ks.iter().any(|k| n.contains(k));
    if let Some(a) = v4 {
        if a.is_link_local() {
            return "169.254.x.x: нет DHCP, интернета нет".into();
        }
    }
    let kind = if matches!(ad.if_type(), IfType::Tunnel)
        || has(&[
            "wintun", "tap-windows", "wireguard", "vpn", "tailscale", "zerotier", "nord",
            "proton", "mullvad", "openvpn", "tunnel",
        ]) {
        "VPN / туннель"
    } else if has(&["vethernet", "hyper-v", "vmware", "virtualbox", "wsl", "docker", "virtual"]) {
        "виртуальный адаптер (VM/WSL/Docker)"
    } else if matches!(ad.if_type(), IfType::Ieee80211) || has(&["wi-fi", "wifi", "wireless", "wlan"]) {
        "Wi-Fi"
    } else if matches!(ad.if_type(), IfType::EthernetCsmacd) {
        "Ethernet"
    } else {
        "сетевой адаптер"
    };
    let gw = if ad.gateways().is_empty() { "без шлюза" } else { "есть шлюз" };
    let scope = match v4 {
        Some(a) if a.is_private() => "частная сеть (NAT)",
        Some(_) => "публичный IPv4",
        None => "только IPv6",
    };
    format!("{kind}, {scope}, {gw}")
}

fn best_v6(ips: &[IpAddr]) -> Option<Ipv6Addr> {
    let cands: Vec<Ipv6Addr> = ips
        .iter()
        .filter_map(|i| match i {
            IpAddr::V6(a)
                if !a.is_loopback()
                    && !a.is_multicast()
                    && (a.segments()[0] & 0xffc0) != 0xfe80 =>
            {
                Some(*a)
            }
            _ => None,
        })
        .collect();
    // глобальный предпочтительнее ULA (fc00::/7)
    cands
        .iter()
        .find(|a| (a.segments()[0] & 0xfe00) != 0xfc00)
        .or_else(|| cands.first())
        .copied()
}

/// Индекс интерфейса (IfIndex) по GUID адаптера вида "{XXXXXXXX-XXXX-...}".
/// ipconfig::Adapter отдаёт только Ipv6IfIndex, который = 0, если IPv6 на адаптере выключен.
fn guid_to_index(name: &str) -> u32 {
    use windows_sys::core::GUID;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ConvertInterfaceGuidToLuid, ConvertInterfaceLuidToIndex,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;

    let h: String = name.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if h.len() != 32 {
        return 0;
    }
    let p = |a: usize, b: usize| u64::from_str_radix(&h[a..b], 16).ok();
    let (Some(d1), Some(d2), Some(d3)) = (p(0, 8), p(8, 12), p(12, 16)) else {
        return 0;
    };
    let mut d4 = [0u8; 8];
    for (i, b) in d4.iter_mut().enumerate() {
        match p(16 + 2 * i, 18 + 2 * i) {
            Some(v) => *b = v as u8,
            None => return 0,
        }
    }
    let guid = GUID { data1: d1 as u32, data2: d2 as u16, data3: d3 as u16, data4: d4 };
    unsafe {
        let mut luid: NET_LUID_LH = std::mem::zeroed();
        if ConvertInterfaceGuidToLuid(&guid, &mut luid) != 0 {
            return 0;
        }
        let mut idx = 0u32;
        if ConvertInterfaceLuidToIndex(&luid, &mut idx) != 0 {
            return 0;
        }
        idx
    }
}

fn list_sources() -> Vec<Source> {
    let mut out = Vec::new();
    for ad in ipconfig::get_adapters().unwrap_or_default() {
        if !matches!(ad.oper_status(), OperStatus::IfOperStatusUp) {
            continue;
        }
        if matches!(ad.if_type(), IfType::SoftwareLoopback) {
            continue;
        }
        let v4s: Vec<Ipv4Addr> = ad
            .ip_addresses()
            .iter()
            .filter_map(|i| match i {
                IpAddr::V4(a) if !a.is_loopback() => Some(*a),
                _ => None,
            })
            .collect();
        let v6 = best_v6(ad.ip_addresses());
        let i4 = guid_to_index(ad.adapter_name());
        let i6 = ad.ipv6_if_index();
        // индекс интерфейса один и тот же для обоих семейств; берём то, что удалось получить
        let (idx4, idx6) = (if i4 != 0 { i4 } else { i6 }, if i6 != 0 { i6 } else { i4 });
        let mk = |v4: Option<Ipv4Addr>| Source {
            name: ad.friendly_name().to_string(),
            label: classify(&ad, v4),
            v4,
            v6,
            idx4,
            idx6,
        };
        if v4s.is_empty() {
            if v6.is_some() {
                out.push(mk(None));
            }
        } else {
            for a in v4s {
                out.push(mk(Some(a)));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ сокеты ---

/// Принудительно привязывает сокет к интерфейсу. Ошибка = отказ (fail-closed).
fn pin(sock: &TcpSocket, idx: u32, v6: bool) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::setsockopt;
    if idx == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "неизвестен индекс интерфейса"));
    }
    // IPv4: индекс в сетевом порядке байт, IPv6: в host-порядке. optname = 31 в обоих случаях.
    let (level, val): (i32, u32) = if v6 { (41, idx) } else { (0, idx.to_be()) };
    let r = unsafe {
        setsockopt(sock.as_raw_socket() as usize, level, 31, &val as *const u32 as *const u8, 4)
    };
    if r != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

async fn try_one(src: &Source, remote: SocketAddr) -> io::Result<TcpStream> {
    let missing = || io::Error::new(io::ErrorKind::AddrNotAvailable, "у адаптера нет адреса этого семейства");
    let (sock, local, idx, v6) = match remote {
        SocketAddr::V4(_) => {
            let a = src.v4.ok_or_else(missing)?;
            (TcpSocket::new_v4()?, SocketAddr::new(IpAddr::V4(a), 0), src.idx4, false)
        }
        SocketAddr::V6(_) => {
            let a = src.v6.ok_or_else(missing)?;
            (TcpSocket::new_v6()?, SocketAddr::new(IpAddr::V6(a), 0), src.idx6, true)
        }
    };
    pin(&sock, idx, v6)?;
    sock.bind(local)?;
    sock.connect(remote).await
}

async fn connect_bound(src: &Source, host: &str, port: u16) -> io::Result<TcpStream> {
    let addrs: Vec<SocketAddr> = lookup_host((host, port)).await?.collect();
    let ordered = addrs
        .iter()
        .filter(|a| a.is_ipv6() && src.v6.is_some())
        .chain(addrs.iter().filter(|a| a.is_ipv4() && src.v4.is_some()));
    let mut last = io::Error::new(io::ErrorKind::Other, "нет подходящих адресов для выбранного адаптера");
    for a in ordered {
        match try_one(src, *a).await {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

// ------------------------------------------------------------------- прокси ---

fn split_host_port(auth: &str, default: u16) -> Option<(String, u16)> {
    if let Some(rest) = auth.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(default);
        return Some((h.to_string(), port));
    }
    match auth.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((auth.to_string(), default)),
    }
}

async fn read_head(c: &mut TcpStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(p + 4);
            return Ok((buf, rest));
        }
        if buf.len() > 65536 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "слишком большой заголовок"));
        }
        let n = c.read(&mut tmp).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Куда выпускать трафик: закрыто (fail-closed), обычным маршрутом, либо через выбранный адаптер.
#[derive(Clone)]
enum Route {
    Closed,
    Direct,
    Bound(Source),
}

#[derive(Clone)]
struct Ctx {
    app: Shared,
    ui: EventLoopProxy<UserEvent>,
}

async fn connect_route(route: &Route, host: &str, port: u16) -> io::Result<TcpStream> {
    match route {
        Route::Closed => Err(io::Error::new(io::ErrorKind::Other, "сеть закрыта: адрес не выбран")),
        Route::Direct => TcpStream::connect((host, port)).await,
        Route::Bound(src) => connect_bound(src, host, port).await,
    }
}

async fn respond(c: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> io::Result<()> {
    let reason = match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    c.write_all(head.as_bytes()).await?;
    c.write_all(body).await?;
    c.flush().await
}

/// API внутренних страниц. Вызывается только из `internal`, где уже проверены заголовки.
fn api_call(name: &str, method: &str, body: &str, ctx: &Ctx) -> Value {
    let post = method.eq_ignore_ascii_case("POST");
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match name {
        "state" => ctx.app.lock().unwrap().state_json(),
        "set" if post => {
            let ok = match (v["key"].as_str(), v.get("value")) {
                (Some(k), Some(val)) => ctx.app.lock().unwrap().set_user(k, val.clone()),
                _ => false,
            };
            if ok {
                let _ = ctx.ui.send_event(UserEvent::SettingsChanged);
            }
            json!({ "ok": ok })
        }
        "bookmark" if post => {
            if let (Some(u), Some("remove")) = (v["url"].as_str(), v["action"].as_str()) {
                ctx.app.lock().unwrap().remove_bookmark(u);
            }
            let _ = ctx.ui.send_event(UserEvent::SettingsChanged);
            json!({ "ok": true })
        }
        "cmd" if post => {
            let _ = ctx.ui.send_event(UserEvent::Cmd(body.to_string()));
            json!({ "ok": true })
        }
        _ => json!({ "error": "unknown" }),
    }
}

/// Внутренние страницы: http://atmo.internal/... Отдаём сами, мимо внешней сети.
async fn internal(
    c: &mut TcpStream,
    method: &str,
    path: &str,
    hdrs: &[&str],
    mut body: Vec<u8>,
    ctx: &Ctx,
) -> io::Result<()> {
    let header = |name: &str| -> Option<String> {
        hdrs.iter().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case(name) {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
    };
    let route_path = path.split('?').next().unwrap_or("/");

    if route_path.starts_with("/favicon") {
        return respond(c, 404, "text/plain", b"").await;
    }
    if let Some(api) = route_path.strip_prefix("/api/") {
        // защита от чужих сайтов: нужен свой заголовок, а для POST ещё и свой Origin
        if header("x-atmo").as_deref() != Some("1") {
            return respond(c, 403, "text/plain", b"forbidden").await;
        }
        if method.eq_ignore_ascii_case("POST") {
            if header("origin").as_deref() != Some("http://atmo.internal") {
                return respond(c, 403, "text/plain", b"forbidden").await;
            }
            let len: usize = header("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
                .min(1 << 20);
            let mut tmp = [0u8; 4096];
            while body.len() < len {
                let n = c.read(&mut tmp).await?;
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&tmp[..n]);
            }
            body.truncate(len);
        }
        let text = String::from_utf8_lossy(&body).to_string();
        let out = api_call(api, method, &text, ctx);
        return respond(c, 200, "application/json; charset=utf-8", out.to_string().as_bytes()).await;
    }
    if route_path == "/play" {
        return respond(c, 200, "text/html; charset=utf-8", game::GAME_HTML.as_bytes()).await;
    }
    respond(c, 200, "text/html; charset=utf-8", pages::PAGES_HTML.as_bytes()).await
}

async fn handle(c: &mut TcpStream, route: Route, ctx: Ctx) -> io::Result<()> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let (head, rest) = read_head(c).await?;
    let head = String::from_utf8_lossy(&head).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, target, ver) = (
        first.next().ok_or_else(|| bad("пустой запрос"))?,
        first.next().ok_or_else(|| bad("нет цели"))?,
        first.next().unwrap_or("HTTP/1.1"),
    );

    // внутренние страницы atmo.internal обслуживаем сами, ещё до проверки маршрута
    if !method.eq_ignore_ascii_case("CONNECT") {
        if let Some(url) = target.strip_prefix("http://") {
            let (auth, path) = match url.find('/') {
                Some(i) => (&url[..i], &url[i..]),
                None => (url, "/"),
            };
            if auth.eq_ignore_ascii_case("atmo.internal") {
                let hdrs: Vec<&str> = lines.clone().filter(|l| !l.is_empty()).collect();
                return internal(c, method, path, &hdrs, rest, &ctx).await;
            }
        }
    }

    let mut up;
    if method.eq_ignore_ascii_case("CONNECT") {
        let (h, p) = split_host_port(target, 443).ok_or_else(|| bad("плохой CONNECT"))?;
        up = connect_route(&route, &h, p).await?;
        c.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        if !rest.is_empty() {
            up.write_all(&rest).await?;
        }
    } else {
        let url = target.strip_prefix("http://").ok_or_else(|| bad("только http:// или CONNECT"))?;
        let (auth, path) = match url.find('/') {
            Some(i) => (&url[..i], &url[i..]),
            None => (url, "/"),
        };
        let (h, p) = split_host_port(auth, 80).ok_or_else(|| bad("плохой хост"))?;
        up = connect_route(&route, &h, p).await?;
        let mut req = format!("{method} {path} {ver}\r\n");
        for l in lines.filter(|l| !l.is_empty()) {
            let low = l.to_ascii_lowercase();
            if low.starts_with("proxy-connection:")
                || low.starts_with("proxy-authorization:")
                || low.starts_with("connection:")
            {
                continue;
            }
            req.push_str(l);
            req.push_str("\r\n");
        }
        req.push_str("Connection: close\r\n\r\n");
        up.write_all(req.as_bytes()).await?;
        if !rest.is_empty() {
            up.write_all(&rest).await?;
        }
    }
    tokio::io::copy_bidirectional(c, &mut up).await?;
    Ok(())
}

async fn serve(mut c: TcpStream, route: Route, ctx: Ctx, mut gen: watch::Receiver<u64>) {
    let work = async {
        if let Err(e) = handle(&mut c, route, ctx).await {
            let msg = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nЗакрыто (fail-closed): {e}"
            );
            let _ = c.write_all(msg.as_bytes()).await;
        }
    };
    tokio::select! {
        _ = gen.changed() => {}   // маршрут сменили: рвём старое соединение
        _ = work => {}
    }
}

// ---------------------------------------------------------------- проверка IP ---

async fn fetch_ip(host: &str, want_v6: bool, src: Option<&Source>) -> String {
    let r = timeout(Duration::from_secs(8), async {
        let addrs: Vec<SocketAddr> =
            lookup_host((host, 80)).await?.filter(|a| a.is_ipv6() == want_v6).collect();
        let a = *addrs
            .first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "у хоста нет такого адреса"))?;
        let mut s = match src {
            Some(src) => try_one(src, a).await?,
            None => TcpStream::connect(a).await?,
        };
        s.write_all(format!("GET / HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
            .await?;
        let mut out = Vec::new();
        s.read_to_end(&mut out).await?;
        let text = String::from_utf8_lossy(&out).to_string();
        Ok::<_, io::Error>(text.split("\r\n\r\n").nth(1).unwrap_or("").trim().to_string())
    })
    .await;
    match r {
        Ok(Ok(s)) if !s.is_empty() => s,
        Ok(Ok(_)) => "пустой ответ".into(),
        Ok(Err(e)) => format!("недоступно ({e})"),
        Err(_) => "таймаут".into(),
    }
}

async fn build_report(src: &Source) -> String {
    let (d4, d6, s4, s6) = tokio::join!(
        fetch_ip("api.ipify.org", false, None),
        fetch_ip("api6.ipify.org", true, None),
        fetch_ip("api.ipify.org", false, Some(src)),
        fetch_ip("api6.ipify.org", true, Some(src)),
    );
    format!(
        "Напрямую (маршрут по умолчанию, обычно VPN):\n  IPv4: {d4}\n  IPv6: {d6}\n\n\
         Через выбранный адаптер ({}):\n  IPv4: {s4}\n  IPv6: {s6}\n\n\
         Если пары совпадают, а ты ждал разные, значит адрес выходит тем же путём.",
        src.name
    )
}

// --------------------------------------------------------------------- UI ---

const TABS_H: f64 = 38.0;
const NAV_H: f64 = 52.0;
const BAR_H: f64 = TABS_H + NAV_H;
const BG: (u8, u8, u8, u8) = (22, 23, 26, 255);

/// Горячие клавиши. Читаем e.code, а не e.key: работает при любой раскладке (в т.ч. русской).
/// Каталог сочетаний и список отключённых подставляет Rust.
const KEYS_JS: &str = r##"(function(){
 if(window.__bbkeys) return; window.__bbkeys=1;
 var MAP=/*KEYMAP*/[], OFF=new Set(/*DISABLED*/[]), BY=Object.create(null);
 MAP.forEach(function(m){ BY[m.c]=m.a; });
 window.__bbSet=function(a){ OFF=new Set(a); };
 var post=function(k){ try{ window.ipc.postMessage('key|'+k); }catch(_){} };
 window.addEventListener('keydown',function(e){
  if(!e.isTrusted||!e.code) return;
  var p=[]; if(e.ctrlKey||e.metaKey) p.push('ctrl'); if(e.altKey) p.push('alt'); if(e.shiftKey) p.push('shift');
  p.push(e.code.toLowerCase());
  var c=p.join('+'); if(!(c in BY)) return;
  if(OFF.has(c)){ e.preventDefault(); e.stopPropagation(); return; }
  if(BY[c]){ e.preventDefault(); e.stopPropagation(); post(BY[c]); }
 },true);
 window.addEventListener('wheel',function(e){
  if(!e.isTrusted||!e.ctrlKey) return;
  e.preventDefault();
  if(!OFF.has('ctrl+wheel')) post(e.deltaY<0?'zoomin':'zoomout');
 },{capture:true,passive:false});
})();"##;

/// Из вкладок (то есть со страниц сайтов) принимаются ТОЛЬКО эти команды-клавиши.
const KEY_ALLOWED: &[&str] = &[
    "newtab", "close", "reopen", "focusurl", "reload", "back", "fwd", "next", "prev",
    "tab1", "tab2", "tab3", "tab4", "tab5", "tab6", "tab7", "tab8", "tab9",
    "bookmark", "bookmarks", "downloads", "settings", "incognito",
    "zoomin", "zoomout", "zoomreset", "fullscreen",
];

const MENU_HTML: &str = r##"<!doctype html><meta charset="utf-8">
<style>
 :root{color-scheme:dark}
 *{box-sizing:border-box}
 html,body{margin:0;height:100%;font:14px "Segoe UI",system-ui,sans-serif;background:#16171a;color:#e8eaed;overflow:hidden}
 button,input{font:inherit;color:inherit;background:#2b2d31;border:1px solid #4a4d55;border-radius:8px;padding:7px 12px}
 button{cursor:pointer} button:hover:not(:disabled){background:#383b41} button:disabled{opacity:.4;cursor:default}
 button.pri{background:#2f6fed;border-color:#2f6fed;color:#fff} button.pri:hover:not(:disabled){background:#4a82f0}
 #welcome,#menu,#chrome{display:none}
 body[data-m=welcome] #welcome{display:flex} body[data-m=menu] #menu{display:flex} body[data-m=browse] #chrome{display:block}
 body.simple #chk2,body.simple #chip{display:none}
 #welcome{height:100%;align-items:center;justify-content:center;flex-direction:column;gap:12px;padding:24px}
 #welcome .logo{font-size:54px;font-weight:200;letter-spacing:.18em;margin-bottom:2px;background:linear-gradient(135deg,#7cc0ff,#a58bff);-webkit-background-clip:text;background-clip:text;color:transparent}
 #welcome .q{color:#9aa0a6;margin-bottom:10px}
 .opt{width:min(560px,100%);text-align:left;padding:16px 18px;border:1px solid #3c3f46;border-radius:12px;background:#1e2024;cursor:pointer}
 .opt:hover{border-color:#2f6fed} .opt b{display:block;font-size:16px;margin-bottom:4px} .opt span{color:#9aa0a6;font-size:13px}
 #tabs{display:flex;align-items:flex-end;gap:2px;height:38px;padding:6px 8px 0;background:#16171a;overflow:hidden}
 .tab{display:flex;align-items:center;gap:6px;min-width:0;flex:0 1 210px;height:32px;padding:0 6px 0 12px;border-radius:8px 8px 0 0;background:#25262a;color:#9aa0a6;cursor:pointer}
 .tab:hover{background:#2d2e33} .tab.on{background:#202124;color:#e8eaed}
 .tab span{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px}
 .tab .x{padding:0 6px;border:0;background:transparent;border-radius:6px;font-size:16px;line-height:20px;color:inherit}
 .tab .x:hover{background:#45464a}
 #plus{align-self:center;padding:0 10px;height:28px;border:0;background:transparent;font-size:18px;line-height:24px}
 #bar{display:flex;gap:6px;align-items:center;padding:8px 10px;height:52px;background:#202124}
 #bar button{padding:6px 10px}
 #nav{display:flex;flex:1;min-width:0} #url{flex:1;min-width:0}
 #chip{white-space:nowrap;max-width:300px;overflow:hidden;text-overflow:ellipsis}
 #zoom:empty{display:none}
 #menu{height:100%;justify-content:center;padding:28px 20px 18px}
 #wrap{width:100%;max-width:780px;display:flex;flex-direction:column;min-height:0}
 h1{margin:0 0 4px;font-size:26px;font-weight:600} .sub{color:#9aa0a6;margin-bottom:14px}
 #list{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;gap:8px;padding:2px}
 .card{padding:12px 14px;border:1px solid #3c3f46;border-radius:10px;background:#1e2024;cursor:pointer}
 .card:hover{border-color:#6b7280} .card.on{border-color:#2f6fed;background:#1c2a44;box-shadow:0 0 0 1px #2f6fed}
 .ip{font-size:16px;font-weight:600}
 .tag{display:inline-block;margin-top:6px;padding:2px 8px;border-radius:999px;font-size:12px;background:#22392a;color:#9be3a2}
 .tag.vpn{background:#3d2f12;color:#f3c767} .tag.virt{background:#2d2f35;color:#aab0bb} .tag.bad{background:#40201f;color:#f2958f}
 .nm{color:#9aa0a6;font-size:12px;margin-top:4px;font-family:Consolas,monospace}
 #foot{display:flex;gap:8px;align-items:center;margin-top:12px} #foot .sp{flex:1}
 #foot label{color:#9aa0a6;font-size:13px;display:flex;align-items:center;gap:6px;cursor:pointer}
 #foot input[type=checkbox]{padding:0;width:15px;height:15px}
 #msg{color:#9aa0a6;font-size:12px;margin-top:8px;min-height:16px}
 #ov{display:none;position:fixed;left:0;top:0;right:0;bottom:0;background:rgba(0,0,0,.6);align-items:center;justify-content:center;padding:20px}
 #ov div{background:#202124;border:1px solid #4a4d55;border-radius:12px;padding:16px;max-width:680px;width:100%}
 #ov pre{white-space:pre-wrap;margin:0 0 12px;font:13px Consolas,monospace}
</style>
<body data-m="%BODY%">
<div id="welcome">
 <div class="logo">atmo</div>
 <div class="q">Для чего тебе браузер? От этого зависит, что мы покажем.</div>
 <div class="opt" id="w1"><b>Обычный браузер</b><span>Вкладки, закладки, загрузки. Никаких лишних настроек сети.</span></div>
 <div class="opt" id="w2"><b>Изоляция сети</b><span>Выбор исходящего адреса, обход блокировок VPN, защита от утечек.</span></div>
 <div class="opt" id="w3"><b>Пока не знаю</b><span>Начнём как с обычного. Режим можно поменять в настройках.</span></div>
</div>
<div id="menu"><div id="wrap">
 <h1>atmo</h1>
 <div class="sub">Выбери адаптер, через который браузер будет выходить в сеть. Пока выбора нет, сеть закрыта.</div>
 <div id="list"></div>
 <div id="foot">
  <button id="rf">Обновить список</button><button id="chk">Проверка IP</button><button id="eye">Показать адреса</button>
  <span class="sp"></span>
  <label><input type="checkbox" id="remember">Запомнить и не спрашивать</label>
  <button id="launch" class="pri" disabled>Открыть браузер →</button>
 </div>
 <div id="msg"></div>
</div></div>
<div id="chrome">
 <div id="tabs"></div>
 <div id="bar">
  <button id="back">←</button><button id="fwd">→</button><button id="rel">⟳</button>
  <form id="nav"><input id="url" placeholder="Поиск или адрес" spellcheck="false"></form>
  <button id="zoom" title="Сбросить масштаб"></button>
  <button id="star" title="В закладки (Ctrl+D)">☆</button>
  <button id="incog" title="Инкогнито-вкладка (Ctrl+Shift+N)">🕶</button>
  <button id="dl" title="Загрузки (Ctrl+J)">⇩</button>
  <button id="bm" title="Закладки (Ctrl+Shift+O)">📑</button>
  <button id="set" title="Настройки (Ctrl+,)">⚙</button>
  <button id="chk2">Проверка IP</button><button id="chip" title="Сменить адрес"></button>
 </div>
</div>
<div id="ov"><div><pre id="rep"></pre><button id="ovx" class="pri">Закрыть</button></div></div>
<script>/*KEYS*/</script>
<script>
 const $=i=>document.getElementById(i);
 const post=m=>{ if(window.ipc) window.ipc.postMessage(m); else $('msg').textContent='IPC недоступен'; };
 let SRC=[],SEL=-1,SHOWIP=false;
 function render(){
  const box=$('list'); box.innerHTML='';
  SRC.forEach((s,i)=>{
   const d=document.createElement('div'); d.className='card'+(i===SEL?' on':'');
   const a=document.createElement('div'); a.className='ip'; a.textContent=s.name; d.appendChild(a);
   const t=document.createElement('span'); t.className='tag '+(s.label.startsWith('VPN')?'vpn':s.label.startsWith('виртуальный')?'virt':s.label.startsWith('169')?'bad':'');
   t.textContent=s.label; d.appendChild(t);
   const n=document.createElement('div'); n.className='nm';
   n.textContent=SHOWIP?((s.v4||'')+(s.v4&&s.v6?'  +  ':'')+(s.v6||'')):'адрес скрыт';
   d.appendChild(n);
   d.onclick=()=>{SEL=i; post('pick|'+i); render();};
   d.ondblclick=()=>post('launch');
   box.appendChild(d);
  });
  $('launch').disabled=SEL<0;
  $('eye').textContent=SHOWIP?'Скрыть адреса':'Показать адреса';
  $('msg').textContent=SRC.length?('Найдено адаптеров: '+SRC.length):'Подходящих адаптеров не найдено. Нажми «Обновить список».';
 }
 function setAddrs(list,sel){SRC=list;SEL=sel;render();}
 function setShowIp(v){SHOWIP=!!v;render();}
 function setUi(o){document.body.classList.toggle('simple',!!o.simple);SHOWIP=!!o.showIp;$('remember').checked=!!o.remember;render();}
 function setTabs(list,act){
  const t=$('tabs'); t.innerHTML='';
  list.forEach((label,i)=>{
   const d=document.createElement('div'); d.className='tab'+(i===act?' on':''); d.title=label;
   const s=document.createElement('span'); s.textContent=label; d.appendChild(s);
   const c=document.createElement('button'); c.className='x'; c.textContent='×'; c.title='Закрыть вкладку';
   c.onclick=e=>{e.stopPropagation(); post('close|'+i);};
   d.appendChild(c);
   d.onclick=()=>post('tab|'+i);
   d.onauxclick=e=>{ if(e.button===1){e.preventDefault(); post('close|'+i);} };
   t.appendChild(d);
  });
  const p=document.createElement('button'); p.id='plus'; p.textContent='+'; p.title='Новая вкладка (Ctrl+T)';
  p.onclick=()=>post('newtab'); t.appendChild(p);
 }
 function setMode(m,chip,full){document.body.dataset.m=m;$('chip').textContent=chip||'';$('chip').title=full||'';}
 function setPage(star,zoom){$('star').textContent=star?'★':'☆';$('zoom').textContent=zoom!==100?(zoom+'%'):'';}
 function focusUrl(){const u=$('url');u.focus();u.select();}
 function setMsg(t){$('msg').textContent=t}
 function setUrl(u){$('url').value=u}
 function showReport(t){ if(document.body.dataset.m==='menu'){$('rep').textContent=t;$('ov').style.display='flex';} else alert(t); }
 $('ovx').onclick=()=>{$('ov').style.display='none'};
 $('w1').onclick=()=>post('mode|simple'); $('w2').onclick=()=>post('mode|advanced'); $('w3').onclick=()=>post('mode|simple');
 $('rf').onclick=()=>post('refresh'); $('chk').onclick=()=>post('check'); $('chk2').onclick=()=>post('check');
 $('eye').onclick=()=>{SHOWIP=!SHOWIP;post('showip|'+(SHOWIP?1:0));render();};
 $('remember').onchange=()=>post('remember|'+($('remember').checked?1:0));
 $('launch').onclick=()=>post('launch'); $('chip').onclick=()=>post('menu');
 $('back').onclick=()=>post('back'); $('fwd').onclick=()=>post('fwd'); $('rel').onclick=()=>post('rel');
 $('star').onclick=()=>post('bookmark'); $('incog').onclick=()=>post('incognito');
 $('dl').onclick=()=>post('downloads'); $('bm').onclick=()=>post('bookmarks'); $('set').onclick=()=>post('settings');
 $('zoom').onclick=()=>post('zoomreset');
 $('nav').onsubmit=e=>{e.preventDefault();post('nav|'+$('url').value)};
 window.addEventListener('error',e=>{$('msg').textContent='JS: '+e.message});
 const INIT=/*INIT*/null;
 setAddrs(INIT||[],-1);
 (function boot(){ if(window.ipc) post('ready'); else setTimeout(boot,50); })();
</script>"##;

struct Tab {
    id: u32,
    view: WebView,
    title: String,
    url: String, // то, что показываем в строке адреса
    raw: String, // настоящий адрес страницы
    incognito: bool,
    zoom: f64,
}

enum UserEvent {
    Ipc(String),
    Url(u32, String),
    Title(u32, String),
    NewTab(String),
    Report(String),
    SettingsChanged,
    Cmd(String),
}

fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn content_rect(window: &Window) -> Rect {
    let s = window.inner_size().to_logical::<f64>(window.scale_factor());
    Rect {
        position: LogicalPosition::new(0.0, BAR_H).into(),
        size: LogicalSize::new(s.width, (s.height - BAR_H).max(1.0)).into(),
    }
}

fn layout(window: &Window, tb: &WebView, tabs: &[Tab], menu: bool) {
    let s = window.inner_size().to_logical::<f64>(window.scale_factor());
    let tb_h = if menu { s.height.max(1.0) } else { BAR_H };
    let _ = tb.set_bounds(Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(s.width, tb_h).into(),
    });
    for t in tabs {
        let _ = t.view.set_bounds(content_rect(window));
    }
}

/// Скрипт горячих клавиш с каталогом сочетаний и текущим списком отключённых.
fn keys_script(app: &Shared) -> String {
    let off = Value::from(app.lock().unwrap().disabled_keys()).to_string();
    KEYS_JS
        .replace("/*KEYMAP*/[]", &keymap().to_string())
        .replace("/*DISABLED*/[]", &off)
}

/// Одна вкладка = один WebView. Новые окна (target=_blank, Ctrl+клик) уходят в новую вкладку.
fn make_tab(
    window: &Window,
    args: &str,
    proxy: &EventLoopProxy<UserEvent>,
    app: &Shared,
    id: u32,
    url: &str,
    incognito: bool,
) -> WebView {
    let accel = app.lock().unwrap().b("accel_keys");
    let keys = keys_script(app);
    let (app_a, app_b) = (app.clone(), app.clone());
    let (p_nav, p_title, p_new, p_key) =
        (proxy.clone(), proxy.clone(), proxy.clone(), proxy.clone());
    WebViewBuilder::new()
        .with_bounds(content_rect(window))
        .with_additional_browser_args(args.to_string())
        .with_background_color(BG)
        .with_incognito(incognito)
        .with_browser_accelerator_keys(accel)
        .with_url(url)
        .with_initialization_script(keys)
        .with_ipc_handler(move |req| {
            // страницы сайтов тоже видят window.ipc: пропускаем только белый список клавиш
            let b = req.body();
            if let Some(k) = b.strip_prefix("key|") {
                if KEY_ALLOWED.contains(&k) {
                    let _ = p_key.send_event(UserEvent::Ipc(b.clone()));
                }
            }
        })
        .with_navigation_handler(move |u| {
            let _ = p_nav.send_event(UserEvent::Url(id, u));
            true
        })
        .with_document_title_changed_handler(move |t| {
            let _ = p_title.send_event(UserEvent::Title(id, t));
        })
        .with_new_window_req_handler(move |u, _features| {
            let _ = p_new.send_event(UserEvent::NewTab(u));
            NewWindowResponse::Deny
        })
        .with_download_started_handler(move |dl_url, path| {
            let mut a = app_a.lock().unwrap();
            let id = a.next_dl;
            a.next_dl += 1;
            a.downloads.push(Download {
                id,
                url: dl_url,
                path: path.to_string_lossy().to_string(),
                state: "running",
            });
            true
        })
        .with_download_completed_handler(move |dl_url, path, ok| {
            let mut a = app_b.lock().unwrap();
            if let Some(d) = a
                .downloads
                .iter_mut()
                .rev()
                .find(|d| d.url == dl_url && d.state == "running")
            {
                d.state = if ok { "done" } else { "failed" };
                if let Some(p) = path {
                    d.path = p.to_string_lossy().to_string();
                }
            }
        })
        .build_as_child(window)
        .unwrap()
}

fn open_tab(
    window: &Window,
    args: &str,
    proxy: &EventLoopProxy<UserEvent>,
    app: &Shared,
    tabs: &mut Vec<Tab>,
    active: &mut usize,
    next_id: &mut u32,
    url: &str,
    incognito: bool,
) {
    let id = *next_id;
    *next_id += 1;
    let view = make_tab(window, args, proxy, app, id, url, incognito);
    tabs.push(Tab {
        id,
        view,
        title: String::new(),
        url: display_url(url),
        raw: if url.starts_with("about:") { String::new() } else { url.to_string() },
        incognito,
        zoom: 1.0,
    });
    *active = tabs.len() - 1;
}

fn sync_views(tabs: &[Tab], active: usize, menu: bool) {
    for (i, t) in tabs.iter().enumerate() {
        let _ = t.view.set_visible(!menu && i == active);
    }
}

fn tab_label(t: &Tab) -> String {
    let base = if !t.title.is_empty() {
        t.title.clone()
    } else if !t.url.is_empty() {
        t.url.clone()
    } else {
        "Новая вкладка".to_string()
    };
    if t.incognito {
        format!("🕶 {base}")
    } else {
        base
    }
}

fn push_tabs(tb: &WebView, tabs: &[Tab], active: usize) {
    let labels: Vec<String> = tabs.iter().map(tab_label).collect();
    let _ = tb.evaluate_script(&format!(
        "setTabs({},{})",
        serde_json::to_string(&labels).unwrap(),
        active
    ));
}

fn set_win_title(window: &Window, tabs: &[Tab], active: usize, menu: bool) {
    let t = match (menu, tabs.get(active)) {
        (false, Some(t)) => format!("{} — atmo", tab_label(t)),
        _ => "atmo".to_string(),
    };
    window.set_title(&t);
}

/// Перерисовать полосу вкладок, строку адреса, звёздочку, масштаб и заголовок окна.
fn refresh_chrome(
    window: &Window,
    tb: &WebView,
    tabs: &[Tab],
    active: usize,
    menu: bool,
    app: &Shared,
) {
    push_tabs(tb, tabs, active);
    let (url, star, zoom) = match tabs.get(active) {
        Some(t) => (
            t.url.clone(),
            app.lock().unwrap().is_bookmarked(&t.raw),
            (t.zoom * 100.0).round() as i32,
        ),
        None => (String::new(), false, 100),
    };
    let _ = tb.evaluate_script(&format!("setUrl({});setPage({star},{zoom})", js(&url)));
    set_win_title(window, tabs, active, menu);
}

/// Перевести клавиатурный фокус в строку адреса (она живёт в панели, а не в странице).
fn focus_url(tb: &WebView) {
    let _ = tb.focus();
    let _ = tb.evaluate_script("focusUrl()");
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Адрес как есть / atmo://страница / https://хост / поиск DuckDuckGo, если это не похоже на адрес.
fn normalize_input(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "about:blank".into();
    }
    if let Some(rest) = t.strip_prefix("atmo://") {
        return format!("http://atmo.internal/{rest}");
    }
    if t.contains("://") || t.starts_with("about:") {
        return t.to_string();
    }
    let looks_host = !t.contains(char::is_whitespace)
        && (t.contains('.') || t.starts_with("localhost") || t.starts_with('['));
    if looks_host {
        format!("https://{t}")
    } else {
        format!("https://duckduckgo.com/?q={}", urlenc(t))
    }
}

/// Что показывать в строке адреса: внутренние страницы как atmo://..., пустая вкладка без текста.
fn display_url(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("http://atmo.internal") {
        let p = rest.split(|c| c == '?' || c == '#').next().unwrap_or("");
        return if p.is_empty() || p == "/" || p == "/newtab" {
            String::new()
        } else {
            format!("atmo:/{p}")
        };
    }
    if raw.starts_with("about:") {
        String::new()
    } else {
        raw.to_string()
    }
}

fn host_of(raw: &str) -> String {
    let r = raw.split("://").nth(1).unwrap_or("");
    let a = r.split(|c| c == '/' || c == '?' || c == '#').next().unwrap_or("");
    let a = a.rsplit('@').next().unwrap_or(a);
    if a.starts_with('[') {
        return a.split(']').next().unwrap_or(a).trim_start_matches('[').to_string();
    }
    a.split(':').next().unwrap_or(a).to_string()
}

fn newtab_url(app: &Shared) -> String {
    if app.lock().unwrap().b("newtab_page") {
        "http://atmo.internal/newtab".to_string()
    } else {
        "about:blank".to_string()
    }
}

fn save_session(app: &Shared, tabs: &[Tab], active: usize) {
    let mut urls = Vec::new();
    let mut act = 0;
    for (i, t) in tabs.iter().enumerate() {
        if t.incognito
            || t.raw.is_empty()
            || t.raw.starts_with("about:")
            || t.raw == "http://atmo.internal/newtab"
        {
            continue;
        }
        if i == active {
            act = urls.len();
        }
        urls.push(t.raw.clone());
    }
    app.lock().unwrap().save_session(urls, act);
}

fn push_keys(tb: &WebView, tabs: &[Tab], app: &Shared) {
    let list = Value::from(app.lock().unwrap().disabled_keys()).to_string();
    let js = format!("window.__bbSet&&window.__bbSet({list})");
    let _ = tb.evaluate_script(&js);
    for t in tabs {
        let _ = t.view.evaluate_script(&js);
    }
}

fn push_ui(tb: &WebView, app: &Shared) {
    let a = app.lock().unwrap();
    let _ = tb.evaluate_script(&format!(
        "setUi({{simple:{},showIp:{},remember:{}}})",
        a.s("mode") != "advanced",
        a.b("show_ip"),
        a.b("remember_adapter")
    ));
}

fn sources_json(sources: &[Source]) -> Value {
    Value::Array(
        sources
            .iter()
            .map(|s| {
                json!({
                    "v4": s.v4.map(|a| a.to_string()),
                    "v6": s.v6.map(|a| a.to_string()),
                    "name": s.name,
                    "label": s.label,
                })
            })
            .collect(),
    )
}

fn push_addrs(tb: &WebView, sources: &[Source], selected: Option<usize>, show_ip: bool) {
    let sel = selected.map(|i| i as i64).unwrap_or(-1);
    let _ = tb.evaluate_script(&format!(
        "setShowIp({show_ip});setAddrs({},{})",
        sources_json(sources),
        sel
    ));
}

fn main() {
    let app: Shared = Arc::new(Mutex::new(App::load()));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let net: Arc<Mutex<Route>> = Arc::new(Mutex::new(Route::Closed));
    let gen_tx = Arc::new(watch::channel(0u64).0);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let ctx = Ctx { app: app.clone(), ui: proxy.clone() };

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    {
        let (net, gen_tx, ctx) = (net.clone(), gen_tx.clone(), ctx.clone());
        rt.spawn(async move {
            loop {
                if let Ok((c, _)) = listener.accept().await {
                    let route = net.lock().unwrap().clone();
                    tokio::spawn(serve(c, route, ctx.clone(), gen_tx.subscribe()));
                }
            }
        });
    }
    let rt_handle = rt.handle().clone();

    // Один и тот же набор аргументов для всех WebView (иначе WebView2 ругается на конфликт окружений).
    let args = format!(
        "--proxy-server=http://127.0.0.1:{port} --disable-quic \
         --force-webrtc-ip-handling-policy=disable_non_proxied_udp --dns-prefetch-disable \
         --disable-features=msSmartScreenProtection,msWebOOUI,msPdfOOUI"
    );

    // Список адаптеров зашиваем прямо в HTML: меню не зависит от IPC при старте.
    let mut sources: Vec<Source> = list_sources();
    let init = sources_json(&sources).to_string().replace("</", "<\\/");
    let start = if app.lock().unwrap().b("first_run_done") { "menu" } else { "welcome" };
    let html = MENU_HTML
        .replace("/*INIT*/null", &init)
        .replace("/*KEYS*/", &keys_script(&app))
        .replace("%BODY%", start);

    let window = WindowBuilder::new()
        .with_title("atmo")
        .with_inner_size(TaoLogicalSize::new(1200.0, 800.0))
        .build(&event_loop)
        .unwrap();

    let init_size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let p_ipc = proxy.clone();
    let toolbar = WebViewBuilder::new()
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(init_size.width, init_size.height.max(1.0)).into(),
        })
        .with_additional_browser_args(args.clone())
        .with_background_color(BG)
        .with_html(html)
        .with_ipc_handler(move |req| {
            let _ = p_ipc.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .build_as_child(&window)
        .unwrap();

    // Вкладок нет, пока не выбран маршрут: первая вкладка создаётся при запуске.
    let mut tabs: Vec<Tab> = Vec::new();
    let mut active: usize = 0;
    let mut next_id: u32 = 0;
    let mut selected: Option<usize> = None;
    let mut menu = true; // true = панель на всё окно (приветствие или выбор адреса)
    let mut booted = false;
    let mut closed: Vec<String> = Vec::new();

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        macro_rules! rc {
            () => {
                refresh_chrome(&window, &toolbar, &tabs, active, menu, &app)
            };
        }
        macro_rules! open {
            ($u:expr, $inc:expr) => {
                open_tab(&window, &args, &proxy, &app, &mut tabs, &mut active, &mut next_id, $u, $inc)
            };
        }
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if !menu {
                    save_session(&app, &tabs, active);
                }
                *flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                layout(&window, &toolbar, &tabs, menu)
            }
            Event::UserEvent(UserEvent::Url(id, u)) => {
                if let Some(pos) = tabs.iter().position(|t| t.id == id) {
                    tabs[pos].url = display_url(&u);
                    tabs[pos].raw = u.clone();
                    let z = app.lock().unwrap().zoom_for(&host_of(&u));
                    if (tabs[pos].zoom - z).abs() > 0.001 {
                        tabs[pos].zoom = z;
                        let _ = tabs[pos].view.zoom(z);
                    }
                    if !tabs[pos].incognito {
                        save_session(&app, &tabs, active);
                    }
                    if pos == active {
                        rc!();
                    } else {
                        push_tabs(&toolbar, &tabs, active);
                    }
                }
            }
            Event::UserEvent(UserEvent::Title(id, t)) => {
                if let Some(tab) = tabs.iter_mut().find(|x| x.id == id) {
                    tab.title = t;
                }
                push_tabs(&toolbar, &tabs, active);
                set_win_title(&window, &tabs, active, menu);
            }
            Event::UserEvent(UserEvent::NewTab(u)) => {
                if !menu {
                    open!(&u, false);
                    sync_views(&tabs, active, menu);
                    rc!();
                }
            }
            Event::UserEvent(UserEvent::Report(r)) => {
                let _ = toolbar.evaluate_script(&format!("showReport({})", js(&r)));
            }
            Event::UserEvent(UserEvent::SettingsChanged) => {
                push_keys(&toolbar, &tabs, &app);
                push_ui(&toolbar, &app);
                let show = app.lock().unwrap().b("show_ip");
                push_addrs(&toolbar, &sources, selected, show);
                rc!();
            }
            Event::UserEvent(UserEvent::Cmd(raw)) => {
                let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                let id = v["id"].as_u64().unwrap_or(0) as u32;
                match v["cmd"].as_str().unwrap_or("") {
                    "welcome_reset" => app.lock().unwrap().set("first_run_done", json!(false)),
                    "dl_open" => {
                        let p = app.lock().unwrap().dl_path(id);
                        if let Some(p) = p {
                            let _ = std::process::Command::new("explorer").arg(p).spawn();
                        }
                    }
                    "dl_folder" => {
                        let p = app.lock().unwrap().dl_path(id);
                        if let Some(p) = p {
                            let _ = std::process::Command::new("explorer")
                                .arg(format!("/select,{p}"))
                                .spawn();
                        }
                    }
                    "dl_clear" => app.lock().unwrap().downloads.retain(|d| d.state == "running"),
                    _ => {}
                }
            }
            Event::UserEvent(UserEvent::Ipc(msg)) => {
                let (cmd, arg) = msg.split_once('|').unwrap_or((msg.as_str(), ""));
                // горячие клавиши превращаем в обычные команды (в меню игнорируем)
                let n = tabs.len().max(1);
                let mapped: Option<String> = if cmd == "key" {
                    Some(if menu {
                        String::new()
                    } else {
                        match arg {
                            "close" => format!("close|{active}"),
                            "reload" => "rel".into(),
                            "next" => format!("tab|{}", (active + 1) % n),
                            "prev" => format!("tab|{}", (active + n - 1) % n),
                            t if t.starts_with("tab") => {
                                let k = t[3..].parse::<usize>().unwrap_or(1).max(1);
                                let i = if k == 9 { n - 1 } else { k - 1 };
                                format!("tab|{i}")
                            }
                            other => other.to_string(),
                        }
                    })
                } else {
                    None
                };
                let (cmd, arg) = match &mapped {
                    Some(m) => m.split_once('|').unwrap_or((m.as_str(), "")),
                    None => (cmd, arg),
                };
                // пока открыта панель на всё окно (приветствие / выбор адреса), доступно только её управление
                if menu
                    && !matches!(
                        cmd,
                        "ready" | "refresh" | "pick" | "launch" | "mode" | "showip"
                            | "remember" | "check" | "autostart" | "menu"
                    )
                {
                    return;
                }
                match cmd {
                    "ready" | "refresh" => {
                        let prev = selected.map(|i| sources[i].clone());
                        sources = list_sources();
                        selected = prev.and_then(|p| sources.iter().position(|s| s.same_as(&p)));
                        let show = app.lock().unwrap().b("show_ip");
                        push_addrs(&toolbar, &sources, selected, show);
                        push_ui(&toolbar, &app);
                        if !booted {
                            booted = true;
                            let _ = proxy.send_event(UserEvent::Ipc("autostart".into()));
                        }
                    }
                    "autostart" => {
                        let (done, mode, remember, last) = {
                            let a = app.lock().unwrap();
                            (
                                a.b("first_run_done"),
                                a.s("mode"),
                                a.b("remember_adapter"),
                                a.settings.get("last_adapter").cloned(),
                            )
                        };
                        if done {
                            if mode != "advanced" {
                                let _ = proxy.send_event(UserEvent::Ipc("launch|direct".into()));
                            } else if remember {
                                if let Some(l) = last {
                                    let name = l[0].as_str().unwrap_or("");
                                    let v4 = l[1].as_str();
                                    let found = sources.iter().position(|s| {
                                        s.name == name && s.v4.map(|a| a.to_string()).as_deref() == v4
                                    });
                                    if let Some(i) = found {
                                        selected = Some(i);
                                        let show = app.lock().unwrap().b("show_ip");
                                        push_addrs(&toolbar, &sources, selected, show);
                                        let _ = proxy.send_event(UserEvent::Ipc("launch|".into()));
                                    }
                                }
                            }
                        }
                    }
                    "pick" => {
                        selected = arg.parse::<usize>().ok().filter(|i| *i < sources.len());
                    }
                    "mode" => {
                        {
                            let mut a = app.lock().unwrap();
                            a.set("mode", json!(arg));
                            a.set("first_run_done", json!(true));
                        }
                        push_ui(&toolbar, &app);
                        if arg == "advanced" {
                            let _ = toolbar.evaluate_script("setMode('menu','','')");
                        } else {
                            let _ = proxy.send_event(UserEvent::Ipc("launch|direct".into()));
                        }
                    }
                    "showip" => {
                        app.lock().unwrap().set("show_ip", json!(arg == "1"));
                        push_ui(&toolbar, &app);
                    }
                    "remember" => {
                        app.lock().unwrap().set("remember_adapter", json!(arg == "1"));
                    }
                    "launch" => {
                        let route = if arg == "direct" {
                            Some(Route::Direct)
                        } else {
                            selected.map(|i| Route::Bound(sources[i].clone()))
                        };
                        match route {
                            Some(route) => {
                                *net.lock().unwrap() = route.clone();
                                app.lock().unwrap().route = match &route {
                                    Route::Bound(s) => format!("Через адаптер «{}»", s.name),
                                    Route::Direct => "Обычный маршрут".to_string(),
                                    Route::Closed => "Закрыта".to_string(),
                                };
                                gen_tx.send_modify(|g| *g += 1);
                                menu = false;
                                let (show, remember) = {
                                    let a = app.lock().unwrap();
                                    (a.b("show_ip"), a.b("remember_adapter"))
                                };
                                if let (Route::Bound(s), true) = (&route, remember) {
                                    app.lock().unwrap().set(
                                        "last_adapter",
                                        json!([s.name, s.v4.map(|a| a.to_string())]),
                                    );
                                }
                                if tabs.is_empty() {
                                    let (restore, urls, act) = {
                                        let a = app.lock().unwrap();
                                        (a.b("restore_session"), a.session.0.clone(), a.session.1)
                                    };
                                    if restore && !urls.is_empty() {
                                        for u in &urls {
                                            open!(u, false);
                                        }
                                        active = act.min(tabs.len() - 1);
                                    } else {
                                        let u = newtab_url(&app);
                                        open!(&u, false);
                                    }
                                } else {
                                    // адрес сменили: перезагружаем все вкладки через новый маршрут
                                    for t in &tabs {
                                        let _ = t.view.evaluate_script("location.reload()");
                                    }
                                }
                                sync_views(&tabs, active, menu);
                                layout(&window, &toolbar, &tabs, menu);
                                let (chip, tip) = match &route {
                                    Route::Bound(s) => {
                                        let ip = s
                                            .v4
                                            .map(|a| a.to_string())
                                            .or(s.v6.map(|a| a.to_string()))
                                            .unwrap_or_default();
                                        let kind = s.label.split(',').next().unwrap_or("").to_string();
                                        let chip = if show {
                                            format!("🟢 {ip} · {kind}")
                                        } else {
                                            format!("🟢 {kind} · {}", s.name)
                                        };
                                        (chip, format!("{}\nНажми, чтобы сменить адрес", s.title(show)))
                                    }
                                    _ => (
                                        "🟢 Обычная сеть".to_string(),
                                        "Трафик идёт обычным маршрутом".to_string(),
                                    ),
                                };
                                let _ = toolbar.evaluate_script(&format!(
                                    "setMode('browse',{},{})",
                                    js(&chip),
                                    js(&tip)
                                ));
                                push_ui(&toolbar, &app);
                                rc!();
                                if tabs.get(active).map(|t| t.url.is_empty()).unwrap_or(false) {
                                    focus_url(&toolbar);
                                }
                            }
                            None => {
                                let _ = toolbar.evaluate_script("setMsg('Сначала выбери адрес.')");
                            }
                        }
                    }
                    "menu" => {
                        // возвращаясь в меню, закрываем сеть (fail-closed) и рвём соединения
                        *net.lock().unwrap() = Route::Closed;
                        app.lock().unwrap().route = "Закрыта".to_string();
                        gen_tx.send_modify(|g| *g += 1);
                        menu = true;
                        sync_views(&tabs, active, menu);
                        layout(&window, &toolbar, &tabs, menu);
                        let _ = toolbar.evaluate_script("setMode('menu','','')");
                        set_win_title(&window, &tabs, active, menu);
                    }
                    "newtab" | "incognito" => {
                        let u = newtab_url(&app);
                        open!(&u, cmd == "incognito");
                        sync_views(&tabs, active, menu);
                        rc!();
                        focus_url(&toolbar);
                    }
                    "tab" => {
                        if let Some(i) = arg.parse::<usize>().ok().filter(|i| *i < tabs.len()) {
                            active = i;
                            sync_views(&tabs, active, menu);
                            rc!();
                            let _ = tabs[active].view.focus();
                        }
                    }
                    "close" => {
                        if let Some(i) = arg.parse::<usize>().ok().filter(|i| *i < tabs.len()) {
                            let keep_open = app.lock().unwrap().b("keep_open");
                            if tabs.len() == 1 && !keep_open {
                                app.lock().unwrap().save_session(Vec::new(), 0);
                                *flow = ControlFlow::Exit;
                            } else {
                                if !tabs[i].raw.is_empty()
                                    && !tabs[i].incognito
                                    && tabs[i].raw != "http://atmo.internal/newtab"
                                {
                                    closed.push(tabs[i].raw.clone());
                                }
                                tabs.remove(i);
                                if tabs.is_empty() {
                                    let u = newtab_url(&app);
                                    open!(&u, false);
                                } else if i < active || active >= tabs.len() {
                                    active = active.saturating_sub(1).min(tabs.len() - 1);
                                }
                                sync_views(&tabs, active, menu);
                                save_session(&app, &tabs, active);
                                rc!();
                            }
                        }
                    }
                    "reopen" => {
                        if let Some(u) = closed.pop() {
                            open!(&u, false);
                            sync_views(&tabs, active, menu);
                            rc!();
                        }
                    }
                    "focusurl" => focus_url(&toolbar),
                    "nav" => {
                        let u = normalize_input(arg);
                        if let Some(t) = tabs.get(active) {
                            let _ = t.view.load_url(&u);
                            let _ = t.view.focus();
                        }
                    }
                    "back" => {
                        if let Some(t) = tabs.get(active) {
                            let _ = t.view.evaluate_script("history.back()");
                        }
                    }
                    "fwd" => {
                        if let Some(t) = tabs.get(active) {
                            let _ = t.view.evaluate_script("history.forward()");
                        }
                    }
                    "rel" => {
                        if let Some(t) = tabs.get(active) {
                            let _ = t.view.evaluate_script("location.reload()");
                        }
                    }
                    "bookmark" => {
                        if let Some(t) = tabs.get(active) {
                            if !t.raw.is_empty() && !t.raw.starts_with("http://atmo.internal") {
                                let title = tab_label(t);
                                app.lock().unwrap().toggle_bookmark(&title, &t.raw);
                            }
                        }
                        rc!();
                    }
                    "bookmarks" | "downloads" | "settings" | "about" => {
                        let url = format!("http://atmo.internal/{cmd}");
                        if let Some(i) = tabs.iter().position(|t| t.raw == url) {
                            active = i;
                        } else {
                            open!(&url, false);
                        }
                        sync_views(&tabs, active, menu);
                        rc!();
                    }
                    "zoomin" | "zoomout" | "zoomreset" => {
                        let mut host = String::new();
                        let mut z = 1.0;
                        if let Some(t) = tabs.get_mut(active) {
                            z = match cmd {
                                "zoomin" => (t.zoom + 0.1).min(5.0),
                                "zoomout" => (t.zoom - 0.1).max(0.3),
                                _ => 1.0,
                            };
                            z = (z * 10.0).round() / 10.0;
                            t.zoom = z;
                            let _ = t.view.zoom(z);
                            host = host_of(&t.raw);
                        }
                        if !host.is_empty() && host != "atmo.internal" {
                            app.lock().unwrap().set_zoom(&host, z);
                        }
                        rc!();
                    }
                    "fullscreen" => {
                        let on = window.fullscreen().is_some();
                        window.set_fullscreen(if on { None } else { Some(Fullscreen::Borderless(None)) });
                    }
                    "check" => match selected {
                        Some(i) => {
                            let src = sources[i].clone();
                            if !menu {
                                // страница с IP открывается в новой вкладке, чтобы не терять текущую
                                open!("https://api64.ipify.org/", false);
                                sync_views(&tabs, active, menu);
                                rc!();
                            }
                            let p = proxy.clone();
                            rt_handle.spawn(async move {
                                let r = build_report(&src).await;
                                let _ = p.send_event(UserEvent::Report(r));
                            });
                        }
                        None => {
                            let _ = toolbar.evaluate_script("setMsg('Сначала выбери адрес.')");
                        }
                    },
                    _ => {}
                }
            }
            _ => {}
        }
    });
}
