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
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
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

const TABS_H: f64 = 38.0;
const NAV_H: f64 = 52.0;
const BAR_H: f64 = TABS_H + NAV_H;

/// Горячие клавиши. Читаем e.code, а не e.key: работает при любой раскладке (в т.ч. русской).
const KEYS_JS: &str = r#"(function(){
 if(window.__bbkeys) return; window.__bbkeys=1;
 var send=function(k){ try{ window.ipc.postMessage('key|'+k); }catch(_){} };
 window.addEventListener('keydown',function(e){
  if(!e.isTrusted) return;
  var c=e.ctrlKey||e.metaKey, k=e.code, a=null;
  if(c&&!e.altKey&&!e.shiftKey){
   if(k==='KeyT') a='newtab'; else if(k==='KeyW') a='close'; else if(k==='KeyL') a='focusurl';
   else if(k==='KeyR') a='reload'; else if(k==='Tab'||k==='PageDown') a='next'; else if(k==='PageUp') a='prev';
   else if(/^Digit[1-9]$/.test(k)) a='tab'+k.charAt(5);
  } else if(c&&e.shiftKey&&!e.altKey){
   if(k==='Tab') a='prev'; else if(k==='KeyT') a='reopen';
  } else if(e.altKey&&!c&&!e.shiftKey){
   if(k==='ArrowLeft') a='back'; else if(k==='ArrowRight') a='fwd';
  } else if(!c&&!e.altKey&&!e.shiftKey&&k==='F5') a='reload';
  if(a){ e.preventDefault(); e.stopPropagation(); send(a); }
 },true);
})();"#;

/// Из вкладок (то есть со страниц сайтов) принимаются ТОЛЬКО эти команды-клавиши.
const KEY_ALLOWED: &[&str] = &[
    "newtab", "close", "reopen", "focusurl", "reload", "back", "fwd", "next", "prev",
    "tab1", "tab2", "tab3", "tab4", "tab5", "tab6", "tab7", "tab8", "tab9",
];

const MENU_HTML: &str = r#"<!doctype html><meta charset="utf-8">
<style>
 :root{color-scheme:dark}
 *{box-sizing:border-box}
 html,body{margin:0;height:100%;font:14px "Segoe UI",system-ui,sans-serif;background:#16171a;color:#e8eaed;overflow:hidden}
 button,input{font:inherit;color:inherit;background:#2b2d31;border:1px solid #4a4d55;border-radius:8px;padding:7px 12px}
 button{cursor:pointer} button:hover:not(:disabled){background:#383b41} button:disabled{opacity:.4;cursor:default}
 button.pri{background:#2f6fed;border-color:#2f6fed;color:#fff} button.pri:hover:not(:disabled){background:#4a82f0}
 body.menu #chrome{display:none} body.browse #menu{display:none}
 #tabs{display:flex;align-items:flex-end;gap:2px;height:38px;padding:6px 8px 0;background:#16171a;overflow:hidden}
 .tab{display:flex;align-items:center;gap:6px;min-width:0;flex:0 1 210px;height:32px;padding:0 6px 0 12px;border-radius:8px 8px 0 0;background:#25262a;color:#9aa0a6;cursor:pointer}
 .tab:hover{background:#2d2e33} .tab.on{background:#202124;color:#e8eaed}
 .tab span{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px}
 .tab .x{padding:0 6px;border:0;background:transparent;border-radius:6px;font-size:16px;line-height:20px;color:inherit}
 .tab .x:hover{background:#45464a}
 #plus{align-self:center;padding:0 10px;height:28px;border:0;background:transparent;font-size:18px;line-height:24px}
 #bar{display:flex;gap:6px;align-items:center;padding:8px 10px;height:52px;background:#202124}
 #nav{display:flex;flex:1;min-width:0} #url{flex:1;min-width:0}
 #chip{white-space:nowrap;max-width:320px;overflow:hidden;text-overflow:ellipsis}
 #menu{height:100%;display:flex;justify-content:center;padding:28px 20px 18px}
 #wrap{width:100%;max-width:780px;display:flex;flex-direction:column;min-height:0}
 h1{margin:0 0 4px;font-size:26px;font-weight:600} .sub{color:#9aa0a6;margin-bottom:14px}
 #list{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;gap:8px;padding:2px}
 .card{padding:12px 14px;border:1px solid #3c3f46;border-radius:10px;background:#1e2024;cursor:pointer}
 .card:hover{border-color:#6b7280} .card.on{border-color:#2f6fed;background:#1c2a44;box-shadow:0 0 0 1px #2f6fed}
 .ip{font-size:16px;font-weight:600} .v6{font:12px Consolas,monospace;color:#9aa0a6;margin-left:8px}
 .tag{display:inline-block;margin-top:6px;padding:2px 8px;border-radius:999px;font-size:12px;background:#22392a;color:#9be3a2}
 .tag.vpn{background:#3d2f12;color:#f3c767} .tag.virt{background:#2d2f35;color:#aab0bb} .tag.bad{background:#40201f;color:#f2958f}
 .nm{color:#9aa0a6;font-size:12px;margin-top:4px}
 #foot{display:flex;gap:8px;align-items:center;margin-top:12px} #foot .sp{flex:1}
 #msg{color:#9aa0a6;font-size:12px;margin-top:8px;min-height:16px}
 #ov{display:none;position:fixed;left:0;top:0;right:0;bottom:0;background:rgba(0,0,0,.6);align-items:center;justify-content:center;padding:20px}
 #ov div{background:#202124;border:1px solid #4a4d55;border-radius:12px;padding:16px;max-width:680px;width:100%}
 #ov pre{white-space:pre-wrap;margin:0 0 12px;font:13px Consolas,monospace}
</style>
<body class="menu">
<div id="menu"><div id="wrap">
 <h1>Bound Browser</h1>
 <div class="sub">Выбери адрес, с которого браузер будет выходить в сеть. Пока адрес не выбран, сеть закрыта.</div>
 <div id="list"></div>
 <div id="foot">
  <button id="rf">Обновить список</button><button id="chk">Проверка IP</button><span class="sp"></span>
  <button id="launch" class="pri" disabled>Открыть браузер →</button>
 </div>
 <div id="msg"></div>
</div></div>
<div id="chrome">
 <div id="tabs"></div>
 <div id="bar">
  <button id="back">←</button><button id="fwd">→</button><button id="rel">⟳</button>
  <form id="nav"><input id="url" placeholder="Адрес или URL" spellcheck="false"></form>
  <button id="chk2">Проверка IP</button><button id="chip" title="Сменить адрес"></button>
 </div>
</div>
<div id="ov"><div><pre id="rep"></pre><button id="ovx" class="pri">Закрыть</button></div></div>
<script>/*KEYS*/</script>
<script>
 const $=i=>document.getElementById(i);
 const post=m=>{ if(window.ipc) window.ipc.postMessage(m); else $('msg').textContent='IPC недоступен'; };
 let SRC=[],SEL=-1;
 function render(){
  const box=$('list'); box.innerHTML='';
  SRC.forEach((s,i)=>{
   const d=document.createElement('div'); d.className='card'+(i===SEL?' on':'');
   const a=document.createElement('div');
   const ip=document.createElement('span'); ip.className='ip'; ip.textContent=s.v4||s.v6; a.appendChild(ip);
   if(s.v4&&s.v6){const v=document.createElement('span'); v.className='v6'; v.textContent='+ '+s.v6; a.appendChild(v);}
   const t=document.createElement('span'); t.className='tag '+(s.label.startsWith('VPN')?'vpn':s.label.startsWith('виртуальный')?'virt':s.label.startsWith('169')?'bad':'');
   t.textContent=s.label;
   const n=document.createElement('div'); n.className='nm'; n.textContent=s.name;
   d.appendChild(a); d.appendChild(t); d.appendChild(n);
   d.onclick=()=>{SEL=i; post('pick|'+i); render();};
   d.ondblclick=()=>post('launch');
   box.appendChild(d);
  });
  $('launch').disabled=SEL<0;
  $('msg').textContent=SRC.length?('Найдено адресов: '+SRC.length):'Подходящих адресов не найдено. Нажми «Обновить список».';
 }
 function setAddrs(list,sel){SRC=list;SEL=sel;render();}
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
  const p=document.createElement('button'); p.id='plus'; p.textContent='+'; p.title='Новая вкладка';
  p.onclick=()=>post('newtab'); t.appendChild(p);
 }
 function setMode(m,chip,full){document.body.className=m;$('chip').textContent=chip||'';$('chip').title=full||'';}
 function focusUrl(){const u=$('url');u.focus();u.select();}
 function setMsg(t){$('msg').textContent=t}
 function setUrl(u){$('url').value=u}
 function showReport(t){ if(document.body.className==='menu'){$('rep').textContent=t;$('ov').style.display='flex';} else alert(t); }
 $('ovx').onclick=()=>{$('ov').style.display='none'};
 $('rf').onclick=()=>post('refresh'); $('chk').onclick=()=>post('check'); $('chk2').onclick=()=>post('check');
 $('launch').onclick=()=>post('launch'); $('chip').onclick=()=>post('menu');
 $('back').onclick=()=>post('back'); $('fwd').onclick=()=>post('fwd'); $('rel').onclick=()=>post('rel');
 $('nav').onsubmit=e=>{e.preventDefault();post('nav|'+$('url').value)};
 window.addEventListener('error',e=>{$('msg').textContent='JS: '+e.message});
 const INIT=/*INIT*/null;
 setAddrs(INIT||[],-1);
 (function boot(){ if(window.ipc) post('ready'); else setTimeout(boot,50); })();
</script>"#;

struct Tab {
    id: u32,
    view: WebView,
    title: String,
    url: String,
}

enum UserEvent {
    Ipc(String),
    Url(u32, String),
    Title(u32, String),
    NewTab(String),
    Report(String),
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

/// Одна вкладка = один WebView. Новые окна (target=_blank, window.open) уходят в новую вкладку.
fn make_tab(
    window: &Window,
    args: &str,
    proxy: &EventLoopProxy<UserEvent>,
    id: u32,
    url: &str,
) -> WebView {
    let (p_nav, p_title, p_new, p_key) =
        (proxy.clone(), proxy.clone(), proxy.clone(), proxy.clone());
    WebViewBuilder::new()
        .with_bounds(content_rect(window))
        .with_additional_browser_args(args.to_string())
        .with_url(url)
        .with_initialization_script(KEYS_JS)
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
        .build_as_child(window)
        .unwrap()
}

fn open_tab(
    window: &Window,
    args: &str,
    proxy: &EventLoopProxy<UserEvent>,
    tabs: &mut Vec<Tab>,
    active: &mut usize,
    next_id: &mut u32,
    url: &str,
) {
    let id = *next_id;
    *next_id += 1;
    let view = make_tab(window, args, proxy, id, url);
    let shown = if url.starts_with("about:") { String::new() } else { url.to_string() };
    tabs.push(Tab { id, view, title: String::new(), url: shown });
    *active = tabs.len() - 1;
}

fn sync_views(tabs: &[Tab], active: usize, menu: bool) {
    for (i, t) in tabs.iter().enumerate() {
        let _ = t.view.set_visible(!menu && i == active);
    }
}

fn tab_label(t: &Tab) -> String {
    if !t.title.is_empty() {
        t.title.clone()
    } else if !t.url.is_empty() {
        t.url.clone()
    } else {
        "Новая вкладка".to_string()
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
        (false, Some(t)) => format!("{} — Bound Browser", tab_label(t)),
        _ => "Bound Browser".to_string(),
    };
    window.set_title(&t);
}

/// Перерисовать полосу вкладок, строку адреса и заголовок окна под активную вкладку.
fn refresh_chrome(window: &Window, tb: &WebView, tabs: &[Tab], active: usize, menu: bool) {
    push_tabs(tb, tabs, active);
    let url = tabs.get(active).map(|t| t.url.clone()).unwrap_or_default();
    let _ = tb.evaluate_script(&format!("setUrl({})", js(&url)));
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

/// Адрес как есть / https://хост / поиск DuckDuckGo, если это не похоже на адрес.
fn normalize_input(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "about:blank".into();
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

fn sources_json(sources: &[Source]) -> serde_json::Value {
    serde_json::Value::Array(
        sources
            .iter()
            .map(|s| {
                serde_json::json!({
                    "v4": s.v4.map(|a| a.to_string()),
                    "v6": s.v6.map(|a| a.to_string()),
                    "name": s.name,
                    "label": s.label,
                })
            })
            .collect(),
    )
}

fn push_addrs(tb: &WebView, sources: &[Source], selected: Option<usize>) {
    let sel = selected.map(|i| i as i64).unwrap_or(-1);
    let _ = tb.evaluate_script(&format!("setAddrs({},{})", sources_json(sources), sel));
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

    // Один и тот же набор аргументов для всех WebView (иначе WebView2 ругается на конфликт окружений).
    let args = format!(
        "--proxy-server=http://127.0.0.1:{port} --disable-quic \
         --force-webrtc-ip-handling-policy=disable_non_proxied_udp --dns-prefetch-disable \
         --disable-features=msSmartScreenProtection,msWebOOUI,msPdfOOUI"
    );

    // Список адресов зашиваем прямо в HTML: меню не зависит от IPC при старте.
    let mut sources: Vec<Source> = list_sources();
    let init = sources_json(&sources).to_string().replace("</", "<\\/");
    let html = MENU_HTML.replace("/*INIT*/null", &init).replace("/*KEYS*/", KEYS_JS);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Bound Browser")
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
        .with_html(html)
        .with_ipc_handler(move |req| {
            let _ = p_ipc.send_event(UserEvent::Ipc(req.body().clone()));
        })
        .build_as_child(&window)
        .unwrap();

    // Вкладок нет, пока не выбран адрес: первая вкладка создаётся при запуске.
    let mut tabs: Vec<Tab> = Vec::new();
    let mut active: usize = 0;
    let mut next_id: u32 = 0;
    let mut selected: Option<usize> = None;
    let mut menu = true;
    let mut closed: Vec<String> = Vec::new();

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                layout(&window, &toolbar, &tabs, menu)
            }
            Event::UserEvent(UserEvent::Url(id, u)) => {
                let shown = if u.starts_with("about:") { String::new() } else { u };
                if let Some(pos) = tabs.iter().position(|t| t.id == id) {
                    tabs[pos].url = shown.clone();
                    if pos == active {
                        let _ = toolbar.evaluate_script(&format!("setUrl({})", js(&shown)));
                    }
                    push_tabs(&toolbar, &tabs, active);
                }
            }
            Event::UserEvent(UserEvent::Title(id, t)) => {
                if let Some(tab) = tabs.iter_mut().find(|x| x.id == id) {
                    tab.title = t;
                    push_tabs(&toolbar, &tabs, active);
                    set_win_title(&window, &tabs, active, menu);
                }
            }
            Event::UserEvent(UserEvent::NewTab(u)) => {
                if !menu {
                    open_tab(&window, &args, &proxy, &mut tabs, &mut active, &mut next_id, &u);
                    sync_views(&tabs, active, menu);
                    refresh_chrome(&window, &toolbar, &tabs, active, menu);
                }
            }
            Event::UserEvent(UserEvent::Report(r)) => {
                let _ = toolbar.evaluate_script(&format!("showReport({})", js(&r)));
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
                            "newtab" => "newtab".into(),
                            "close" => format!("close|{active}"),
                            "reopen" => "reopen".into(),
                            "focusurl" => "focusurl".into(),
                            "reload" => "rel".into(),
                            "back" => "back".into(),
                            "fwd" => "fwd".into(),
                            "next" => format!("tab|{}", (active + 1) % n),
                            "prev" => format!("tab|{}", (active + n - 1) % n),
                            t if t.starts_with("tab") => {
                                let k = t[3..].parse::<usize>().unwrap_or(1).max(1);
                                let i = if k == 9 { n - 1 } else { k - 1 };
                                format!("tab|{i}")
                            }
                            _ => String::new(),
                        }
                    })
                } else {
                    None
                };
                let (cmd, arg) = match &mapped {
                    Some(m) => m.split_once('|').unwrap_or((m.as_str(), "")),
                    None => (cmd, arg),
                };
                match cmd {
                    "ready" | "refresh" => {
                        let prev = selected.map(|i| sources[i].clone());
                        sources = list_sources();
                        selected = prev.and_then(|p| sources.iter().position(|s| s.same_as(&p)));
                        push_addrs(&toolbar, &sources, selected);
                    }
                    "pick" => {
                        selected = arg.parse::<usize>().ok().filter(|i| *i < sources.len());
                    }
                    "launch" => match selected {
                        Some(i) => {
                            let s = sources[i].clone();
                            *state.lock().unwrap() = Some(s.clone());
                            gen_tx.send_modify(|g| *g += 1);
                            menu = false;
                            if tabs.is_empty() {
                                open_tab(&window, &args, &proxy, &mut tabs, &mut active, &mut next_id, "about:blank");
                            } else {
                                // адрес сменили: перезагружаем все вкладки через новый адаптер
                                for t in &tabs {
                                    let _ = t.view.evaluate_script("location.reload()");
                                }
                            }
                            sync_views(&tabs, active, menu);
                            layout(&window, &toolbar, &tabs, menu);
                            let ip = s
                                .v4
                                .map(|a| a.to_string())
                                .or(s.v6.map(|a| a.to_string()))
                                .unwrap_or_default();
                            let kind = s.label.split(',').next().unwrap_or("").to_string();
                            let _ = toolbar.evaluate_script(&format!(
                                "setMode('browse',{},{})",
                                js(&format!("🟢 {ip} · {kind}")),
                                js(&format!("{}\nНажми, чтобы сменить адрес", s.title()))
                            ));
                            refresh_chrome(&window, &toolbar, &tabs, active, menu);
                            focus_url(&toolbar);
                        }
                        None => {
                            let _ = toolbar.evaluate_script("setMsg('Сначала выбери адрес.')");
                        }
                    },
                    "menu" => {
                        // возвращаясь в меню, закрываем сеть (fail-closed) и рвём соединения
                        *state.lock().unwrap() = None;
                        gen_tx.send_modify(|g| *g += 1);
                        menu = true;
                        sync_views(&tabs, active, menu);
                        layout(&window, &toolbar, &tabs, menu);
                        let _ = toolbar.evaluate_script("setMode('menu','','')");
                        set_win_title(&window, &tabs, active, menu);
                    }
                    "newtab" => {
                        if !menu {
                            open_tab(&window, &args, &proxy, &mut tabs, &mut active, &mut next_id, "about:blank");
                            sync_views(&tabs, active, menu);
                            refresh_chrome(&window, &toolbar, &tabs, active, menu);
                            focus_url(&toolbar);
                        }
                    }
                    "tab" => {
                        if let Some(i) = arg.parse::<usize>().ok().filter(|i| *i < tabs.len()) {
                            active = i;
                            sync_views(&tabs, active, menu);
                            refresh_chrome(&window, &toolbar, &tabs, active, menu);
                            let _ = tabs[active].view.focus();
                        }
                    }
                    "close" => {
                        if let Some(i) = arg.parse::<usize>().ok().filter(|i| *i < tabs.len()) {
                            if !tabs[i].url.is_empty() {
                                closed.push(tabs[i].url.clone());
                            }
                            tabs.remove(i);
                            if tabs.is_empty() {
                                open_tab(&window, &args, &proxy, &mut tabs, &mut active, &mut next_id, "about:blank");
                            } else if i < active || active >= tabs.len() {
                                active = active.saturating_sub(1).min(tabs.len() - 1);
                            }
                            sync_views(&tabs, active, menu);
                            refresh_chrome(&window, &toolbar, &tabs, active, menu);
                        }
                    }
                    "reopen" => {
                        if let Some(u) = closed.pop() {
                            open_tab(&window, &args, &proxy, &mut tabs, &mut active, &mut next_id, &u);
                            sync_views(&tabs, active, menu);
                            refresh_chrome(&window, &toolbar, &tabs, active, menu);
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
                    "check" => match selected {
                        Some(i) => {
                            let src = sources[i].clone();
                            if !menu {
                                // страница с IP открывается в новой вкладке, чтобы не терять текущую
                                open_tab(&window, &args, &proxy, &mut tabs, &mut active, &mut next_id, "https://api64.ipify.org/");
                                sync_views(&tabs, active, menu);
                                refresh_chrome(&window, &toolbar, &tabs, active, menu);
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
