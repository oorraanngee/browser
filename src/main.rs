#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Браузер, который выходит в сеть через выбранный адаптер/адрес (Windows).
//!
//! WebView2 (Chromium) не умеет привязывать исходящие соединения к локальному IP,
//! поэтому внутри процесса поднимается прокси на 127.0.0.1. Он открывает исходящие
//! сокеты с bind() на выбранный адрес + принудительной привязкой к интерфейсу
//! (IP_UNICAST_IF / IPV6_UNICAST_IF). Любая ошибка = соединение закрыто (fail-closed).

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ipconfig::{IfType, OperStatus};
use tao::dpi::LogicalSize as TaoLogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;
use tokio::time::timeout;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows};

const TOOLBAR_H: f64 = 96.0;

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
    fn title(&self) -> String {
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

async fn handle(c: &mut TcpStream, src: Option<Source>) -> io::Result<()> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let src = src.ok_or_else(|| bad("адрес не выбран"))?;
    let (head, rest) = read_head(c).await?;
    let head = String::from_utf8_lossy(&head).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, target, ver) = (
        first.next().ok_or_else(|| bad("пустой запрос"))?,
        first.next().ok_or_else(|| bad("нет цели"))?,
        first.next().unwrap_or("HTTP/1.1"),
    );

    let mut up;
    if method.eq_ignore_ascii_case("CONNECT") {
        let (h, p) = split_host_port(target, 443).ok_or_else(|| bad("плохой CONNECT"))?;
        up = connect_bound(&src, &h, p).await?;
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
        up = connect_bound(&src, &h, p).await?;
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

async fn serve(mut c: TcpStream, src: Option<Source>, mut gen: watch::Receiver<u64>) {
    let work = async {
        if let Err(e) = handle(&mut c, src).await {
            let msg = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nЗакрыто (fail-closed): {e}"
            );
            let _ = c.write_all(msg.as_bytes()).await;
        }
    };
    tokio::select! {
        _ = gen.changed() => {}   // адрес сменили: рвём старое соединение
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

const TOOLBAR_HTML: &str = r#"<!doctype html><meta charset="utf-8">
<style>
 html,body{margin:0;font:13px Segoe UI,sans-serif;background:#202124;color:#e8eaed;overflow:hidden}
 .row{display:flex;gap:6px;padding:6px 8px;align-items:center}
 button,input,select{background:#35363a;color:#e8eaed;border:1px solid #5f6368;border-radius:6px;padding:5px 9px;font:inherit}
 button:hover{background:#45464a} input{flex:1;min-width:0} select{flex:1;min-width:0}
 #st{padding:0 10px 4px;font-size:12px;color:#9aa0a6;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
</style>
<form id="go" class="row">
 <button type="button" id="back">←</button><button type="button" id="fwd">→</button>
 <button type="button" id="rel">⟳</button><input id="url" placeholder="Адрес или URL" spellcheck="false">
</form>
<div class="row"><span>Исходящий адрес:</span><select id="sel"></select>
 <button id="rf">Обновить</button><button id="chk">Проверка IP</button></div>
<div id="st"></div>
<script>
 const ipc=m=>window.ipc.postMessage(m), $=i=>document.getElementById(i);
 $('go').onsubmit=e=>{e.preventDefault();ipc('nav|'+$('url').value)};
 $('back').onclick=()=>ipc('back');$('fwd').onclick=()=>ipc('fwd');$('rel').onclick=()=>ipc('rel');
 $('rf').onclick=()=>ipc('refresh');$('chk').onclick=()=>ipc('check');
 $('sel').onchange=()=>ipc('pick|'+$('sel').value);
 function setAddrs(list,sel){const s=$('sel');s.innerHTML='';
  const o=document.createElement('option');o.value=-1;o.textContent='— выберите адрес (пока не выбран, сеть закрыта) —';s.appendChild(o);
  list.forEach((t,i)=>{const e=document.createElement('option');e.value=i;e.textContent=t;s.appendChild(e)});
  s.value=sel;}
 function setUrl(u){$('url').value=u}
 function setStatus(t){$('st').textContent=t}
 function showReport(t){alert(t)}
 ipc('ready');
</script>"#;

enum UserEvent {
    Ipc(String),
    Url(String),
    Report(String),
}

fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn layout(window: &Window, tb: &WebView, ct: &WebView) {
    let s = window.inner_size().to_logical::<f64>(window.scale_factor());
    let _ = tb.set_bounds(Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(s.width, TOOLBAR_H).into(),
    });
    let _ = ct.set_bounds(Rect {
        position: LogicalPosition::new(0.0, TOOLBAR_H).into(),
        size: LogicalSize::new(s.width, (s.height - TOOLBAR_H).max(1.0)).into(),
    });
}

fn push_addrs(tb: &WebView, sources: &[Source], selected: Option<usize>) {
    let titles: Vec<String> = sources.iter().map(|s| s.title()).collect();
    let sel = selected.map(|i| i as i64).unwrap_or(-1);
    let _ = tb.evaluate_script(&format!(
        "setAddrs({},{})",
        serde_json::to_string(&titles).unwrap(),
        sel
    ));
}

fn push_status(tb: &WebView, src: Option<&Source>) {
    let t = match src {
        None => "🔒 Сеть закрыта: адрес не выбран".to_string(),
        Some(s) => format!(
            "🟢 {} · IPv4 {} · IPv6 {} · привязка к интерфейсу (idx {}/{}) · при ошибке соединения закрываются",
            s.name,
            s.v4.map(|a| a.to_string()).unwrap_or("—".into()),
            s.v6.map(|a| a.to_string()).unwrap_or("—".into()),
            s.idx4,
            s.idx6
        ),
    };
    let _ = tb.evaluate_script(&format!("setStatus({})", js(&t)));
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let state: Arc<Mutex<Option<Source>>> = Arc::new(Mutex::new(None));
    let gen_tx = Arc::new(watch::channel(0u64).0);

    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    {
        let (state, gen_tx) = (state.clone(), gen_tx.clone());
        rt.spawn(async move {
            loop {
                if let Ok((c, _)) = listener.accept().await {
                    let src = state.lock().unwrap().clone();
                    tokio::spawn(serve(c, src, gen_tx.subscribe()));
                }
            }
        });
    }
    let rt_handle = rt.handle().clone();

    // Один и тот же набор аргументов для обоих WebView (иначе WebView2 ругается на конфликт окружений).
    let args = format!(
        "--proxy-server=http://127.0.0.1:{port} --disable-quic \
         --force-webrtc-ip-handling-policy=disable_non_proxied_udp --dns-prefetch-disable \
         --disable-features=msSmartScreenProtection,msWebOOUI,msPdfOOUI"
    );

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Bound Browser")
        .with_inner_size(TaoLogicalSize::new(1200.0, 800.0))
        .build(&event_loop)
        .unwrap();

    let init = window.inner_size().to_logical::<f64>(window.scale_factor());
    let p_ipc = proxy.clone();
    let toolbar = WebViewBuilder::new()
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(init.width, TOOLBAR_H).into(),
        })
        .with_additional_browser_args(args.clone())
        .with_html(TOOLBAR_HTML)
        .with_ipc_handler(move |req| {
            let _ = p_ipc.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .build_as_child(&window)
        .unwrap();

    let p_nav = proxy.clone();
    let content = WebViewBuilder::new()
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, TOOLBAR_H).into(),
            size: LogicalSize::new(init.width, (init.height - TOOLBAR_H).max(1.0)).into(),
        })
        .with_additional_browser_args(args)
        .with_url("about:blank")
        .with_navigation_handler(move |u| {
            let shown = if u.starts_with("about:") { String::new() } else { u };
            let _ = p_nav.send_event(UserEvent::Url(shown));
            true
        })
        .build_as_child(&window)
        .unwrap();

    let mut sources: Vec<Source> = Vec::new();
    let mut selected: Option<usize> = None;

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => layout(&window, &toolbar, &content),
            Event::UserEvent(UserEvent::Url(u)) => {
                let _ = toolbar.evaluate_script(&format!("setUrl({})", js(&u)));
            }
            Event::UserEvent(UserEvent::Report(r)) => {
                let _ = toolbar.evaluate_script(&format!("showReport({})", js(&r)));
            }
            Event::UserEvent(UserEvent::Ipc(msg)) => {
                let (cmd, arg) = msg.split_once('|').unwrap_or((msg.as_str(), ""));
                match cmd {
                    "ready" | "refresh" => {
                        let prev = selected.map(|i| sources[i].clone());
                        sources = list_sources();
                        selected = prev.and_then(|p| sources.iter().position(|s| s.same_as(&p)));
                        *state.lock().unwrap() = selected.map(|i| sources[i].clone());
                        gen_tx.send_modify(|g| *g += 1);
                        push_addrs(&toolbar, &sources, selected);
                        push_status(&toolbar, selected.map(|i| &sources[i]));
                    }
                    "pick" => {
                        selected = arg.parse::<usize>().ok().filter(|i| *i < sources.len());
                        *state.lock().unwrap() = selected.map(|i| sources[i].clone());
                        gen_tx.send_modify(|g| *g += 1);
                        push_status(&toolbar, selected.map(|i| &sources[i]));
                        if selected.is_some() {
                            let _ = content.evaluate_script("location.reload()");
                        }
                    }
                    "nav" => {
                        let mut u = arg.trim().to_string();
                        if !u.contains("://") {
                            u = format!("https://{u}");
                        }
                        let _ = content.load_url(&u);
                    }
                    "back" => { let _ = content.evaluate_script("history.back()"); }
                    "fwd" => { let _ = content.evaluate_script("history.forward()"); }
                    "rel" => { let _ = content.evaluate_script("location.reload()"); }
                    "check" => match selected {
                        Some(i) => {
                            let src = sources[i].clone();
                            let _ = content.load_url("https://api64.ipify.org/");
                            let p = proxy.clone();
                            rt_handle.spawn(async move {
                                let r = build_report(&src).await;
                                let _ = p.send_event(UserEvent::Report(r));
                            });
                        }
                        None => {
                            let _ = toolbar.evaluate_script("showReport('Сначала выбери адрес.')");
                        }
                    },
                    _ => {}
                }
            }
            _ => {}
        }
    });
}
