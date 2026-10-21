//! Внутренние страницы atmo одним бандлом. Прокси отдаёт его по http://atmo.internal/<страница>.

pub const PAGES_HTML: &str = r##"<!doctype html>
<html lang="ru"><head><meta charset="utf-8">
<meta name="color-scheme" content="dark">
<title>atmo</title>
<style>
:root{--bg:#16171a;--panel:#1e2024;--line:#33363d;--txt:#e8eaed;--mut:#9aa0a6;--acc:#4f8cff;--h:215;color-scheme:dark}
*{box-sizing:border-box}
html,body{margin:0;min-height:100%;background:var(--bg);color:var(--txt);font:14px "Segoe UI",system-ui,sans-serif}
a{color:inherit;text-decoration:none}
button,select{font:inherit;color:inherit;background:#2b2d31;border:1px solid #4a4d55;border-radius:8px;padding:7px 12px;cursor:pointer}
button:hover{background:#383b41} button.pri{background:var(--acc);border-color:var(--acc);color:#fff}
.wrap{max-width:820px;margin:0 auto;padding:32px 20px 60px}
h1{font-size:24px;font-weight:600;margin:0 0 18px}
h2{font-size:12px;letter-spacing:.09em;text-transform:uppercase;color:var(--mut);margin:26px 0 8px;font-weight:600}
.row{display:flex;align-items:center;gap:12px;padding:11px 14px;background:var(--panel);border:1px solid var(--line);border-radius:10px;margin-bottom:6px}
.row .tx{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis}
.d{color:var(--mut);font-size:12px;margin-top:2px}
kbd{font:12px Consolas,monospace;background:#2b2d31;border:1px solid #4a4d55;border-radius:5px;padding:2px 7px;color:var(--mut);min-width:96px;text-align:center}
.sw{position:relative;width:40px;height:22px;flex:none}
.sw input{opacity:0;width:100%;height:100%;margin:0;position:absolute;cursor:pointer;z-index:1}
.sw i{position:absolute;left:0;top:0;right:0;bottom:0;background:#3a3d44;border-radius:11px;transition:.15s}
.sw i:after{content:"";position:absolute;left:3px;top:3px;width:16px;height:16px;border-radius:50%;background:#cfd2d8;transition:.15s}
.sw input:checked+i{background:var(--acc)} .sw input:checked+i:after{left:21px;background:#fff}
.nt{min-height:100vh;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:26px;padding:20px}
.logo{font-size:64px;font-weight:200;letter-spacing:.18em;user-select:none;cursor:default;
 background:linear-gradient(135deg,hsl(var(--h) 90% 70%),hsl(calc(var(--h) + 60) 90% 62%));-webkit-background-clip:text;background-clip:text;color:transparent}
.sb{width:min(560px,90vw)}
.sb input{width:100%;padding:13px 18px;border-radius:26px;border:1px solid var(--line);background:var(--panel);color:var(--txt);font:inherit;font-size:15px;outline:none}
.sb input:focus{border-color:var(--acc)}
.tiles{display:flex;flex-wrap:wrap;gap:14px;justify-content:center;max-width:640px}
.tile{width:92px;text-align:center}
.ic{width:52px;height:52px;margin:0 auto 6px;border-radius:14px;background:var(--panel);border:1px solid var(--line);display:flex;align-items:center;justify-content:center;font-size:22px}
.tt{font-size:12px;color:var(--mut);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.roll{animation:roll 1.1s ease} @keyframes roll{to{transform:rotate(360deg)}}
.fall{animation:fall 1.2s cubic-bezier(.5,0,1,.6) forwards} @keyframes fall{to{transform:translateY(60vh) rotate(25deg);opacity:0}}
.aurora .logo{animation:hue 6s linear infinite} @keyframes hue{to{filter:hue-rotate(360deg)}}
.ov{position:fixed;left:0;top:0;right:0;bottom:0;background:#0b0c0e;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:12px;z-index:9}
.big{font-size:44px;font-weight:200;letter-spacing:.14em}
.state{font-size:12px;padding:2px 8px;border-radius:999px;background:#22392a;color:#9be3a2}
.state.running{background:#3d2f12;color:#f3c767} .state.failed{background:#40201f;color:#f2958f}
.empty{color:var(--mut);padding:24px 0}

.wrap{animation:in .35s ease both} .row{animation:in .35s ease both}
@keyframes in{from{opacity:0;transform:translateY(8px)}to{opacity:1;transform:none}}
.nt>*{animation:in .5s ease both} .nt>*:nth-child(2){animation-delay:.08s} .nt>*:nth-child(3){animation-delay:.16s}
button,select,.tile .ic,.row{transition:background .15s,border-color .15s,transform .15s}
button:active{transform:scale(.97)} .tile:hover .ic{transform:translateY(-3px);border-color:var(--acc)}
.hero{text-align:center;padding:30px 0 22px}
.logo2{font-size:58px;font-weight:200;letter-spacing:.2em;background:linear-gradient(135deg,hsl(var(--h) 90% 72%),hsl(calc(var(--h) + 60) 90% 64%));-webkit-background-clip:text;background-clip:text;color:transparent;animation:in .6s ease both}
.ver{position:relative;overflow:hidden;margin:10px 0 8px;padding:8px 20px;border-radius:999px;border:1px solid var(--line);background:var(--panel);cursor:pointer;user-select:none;font-size:14px}
.ver:hover{border-color:var(--acc)}
.ver.hit{animation:squish .28s cubic-bezier(.3,1.6,.5,1)}
@keyframes squish{0%{transform:scale(.9)}60%{transform:scale(1.05)}100%{transform:scale(1)}}
.rip{position:absolute;width:12px;height:12px;margin:-6px 0 0 -6px;border-radius:50%;background:rgba(90,169,255,.55);animation:rip .65s ease-out forwards;pointer-events:none}
@keyframes rip{to{transform:scale(22);opacity:0}}
.box{background:var(--panel);border:1px solid var(--line);border-radius:14px;overflow:hidden}
.kv{display:flex;gap:14px;justify-content:space-between;padding:11px 16px;border-bottom:1px solid var(--line);animation:in .35s ease both}
.kv:last-child{border-bottom:0} .kv .k{color:var(--mut)} .kv .val{text-align:right;word-break:break-all}
#toast{position:fixed;left:50%;bottom:34px;transform:translate(-50%,16px);opacity:0;padding:10px 18px;border-radius:12px;background:#2b2d33;border:1px solid #44474f;transition:.28s;pointer-events:none;z-index:20}
#toast.on{opacity:1;transform:translate(-50%,0)}
body{transition:opacity .6s ease,filter .6s ease} body.fade{opacity:0;filter:blur(6px)}
@media (prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important}}
</style></head>
<body><div id="app"></div>
<script>
const $=(s,r=document)=>r.querySelector(s);
const el=(t,c,x)=>{const e=document.createElement(t);if(c)e.className=c;if(x!=null)e.textContent=x;return e};
const api=(p,b)=>fetch('/api/'+p,{method:b?'POST':'GET',headers:{'X-Atmo':'1'},body:b?JSON.stringify(b):undefined}).then(r=>r.json());
let S={settings:{},bookmarks:[],downloads:[],keymap:[],defaults:{},version:''};
const view=location.pathname.replace(/^\/+/,'').split('/')[0]||'newtab';
const V={newtab,settings,bookmarks,downloads,about};

function sw(on,fn){const l=el('label','sw');const i=el('input');i.type='checkbox';i.checked=!!on;i.onchange=()=>fn(i.checked);l.append(i,el('i'));return l}
function row(t,d,ctl){const r=el('div','row');const x=el('div','tx');x.append(el('div','',t));if(d)x.append(el('div','d',d));r.append(x);if(ctl)r.append(ctl);return r}
function setv(k,v){S.settings[k]=v;return api('set',{key:k,value:v})}
function flag(k){return k in S.settings?!!S.settings[k]:!!S.defaults[k]}
function cmd(c,o){return api('cmd',Object.assign({cmd:c},o||{}))}

async function load(){S=await api('state');(V[view]||newtab)($('#app'))}

/* ---------- новая вкладка ---------- */
let logoN=0;
function go(q){
 q=q.trim();if(!q)return;
 const l=q.toLowerCase();
 if(l==='do a barrel roll'){document.body.classList.add('roll');setTimeout(()=>document.body.classList.remove('roll'),1200);return}
 if(l==='/gravity'){$('.logo').classList.add('fall');return}
 if(/^[a-z][a-z0-9+.-]*:\/\//i.test(q)){location.href=q.replace(/^atmo:\/\//i,'/');return}
 if(/^[^\s]+\.[^\s]+$/.test(q)){location.href='https://'+q;return}
 location.href='https://duckduckgo.com/?q='+encodeURIComponent(q);
}
function newtab(r){
 document.title='Новая вкладка';r.innerHTML='';
 const w=el('div','nt');
 const logo=el('div','logo','atmo');
 logo.onclick=()=>{logoN++;if(logoN%5===0)document.documentElement.style.setProperty('--h',Math.floor(Math.random()*360))};
 const f=el('form','sb');const i=el('input');i.placeholder='Поиск или адрес';i.autocomplete='off';i.spellcheck=false;f.append(i);
 f.onsubmit=e=>{e.preventDefault();go(i.value)};
 const tiles=el('div','tiles');
 S.bookmarks.slice(0,8).forEach(b=>{
  const a=el('a','tile');a.href=b.url;
  a.append(el('div','ic',(b.title||b.url).trim().charAt(0).toUpperCase()),el('div','tt',b.title||b.url));
  tiles.append(a)});
 w.append(logo,f,tiles);r.append(w);setTimeout(()=>i.focus(),60);
}

/* ---------- настройки ---------- */
function settings(r){
 document.title='Настройки';r.innerHTML='';
 const w=el('div','wrap');w.append(el('h1','','Настройки'));

 w.append(el('h2','','Общие'));
 const sel=el('select');
 [['simple','Обычный браузер'],['advanced','Изоляция сети (выбор адреса)']].forEach(([v,t])=>{const o=el('option','',t);o.value=v;sel.append(o)});
 sel.value=S.settings.mode==='advanced'?'advanced':'simple';sel.onchange=()=>setv('mode',sel.value);
 w.append(row('Режим','Применится после перезапуска. «Изоляция сети» показывает выбор исходящего адреса при запуске.',sel));
 w.append(row('Показывать IP-адреса','По умолчанию адреса скрыты: видны только тип и имя адаптера.',sw(flag('show_ip'),v=>setv('show_ip',v))));
 w.append(row('Запоминать адаптер','Не спрашивать адрес при запуске (режим изоляции).',sw(flag('remember_adapter'),v=>setv('remember_adapter',v))));

 w.append(el('h2','','Запуск и вкладки'));
 w.append(row('Страница новой вкладки','Если выключить, новая вкладка будет пустой.',sw(flag('newtab_page'),v=>setv('newtab_page',v))));
 w.append(row('Восстанавливать вкладки','Открывать при запуске те вкладки, что были закрыты вместе с браузером.',sw(flag('restore_session'),v=>setv('restore_session',v))));
 w.append(row('Не закрывать окно с последней вкладкой','Вместо выхода откроется новая вкладка.',sw(flag('keep_open'),v=>setv('keep_open',v))));

 w.append(el('h2','','Горячие клавиши'));
 w.append(row('Встроенные сочетания Chromium','Все разом: F12, Ctrl+U, Ctrl+P и т. д. Применится после перезапуска.',sw(flag('accel_keys'),v=>setv('accel_keys',v))));
 const off=new Set(S.settings.disabled_keys||[]);
 S.keymap.forEach(m=>{
  const r2=el('div','row');r2.append(el('kbd','',m.l));r2.append(el('div','tx',m.d));
  r2.append(sw(!off.has(m.c),on=>{on?off.delete(m.c):off.add(m.c);setv('disabled_keys',[...off])}));
  w.append(r2)});

 w.append(el('h2','','Прочее'));
 const b1=el('button','','Показать приветствие при следующем запуске');b1.onclick=()=>{cmd('welcome_reset');b1.textContent='Готово'};
 w.append(row('Приветствие','Заново спросить, для чего нужен браузер.',b1));
 const a=el('a');a.href='/about';const b2=el('button','','О atmo');a.append(b2);
 w.append(row('О программе',null,a));
 r.append(w);
}

/* ---------- закладки ---------- */
function bookmarks(r){
 document.title='Закладки';r.innerHTML='';
 const w=el('div','wrap');w.append(el('h1','','Закладки'));
 if(!S.bookmarks.length)w.append(el('div','empty','Пока пусто. Нажми ☆ в адресной строке или Ctrl+D.'));
 S.bookmarks.forEach(b=>{
  const a=el('a','tx');a.href=b.url;a.append(el('div','',b.title||b.url),el('div','d',b.url));
  const x=el('button','','Удалить');x.onclick=async()=>{await api('bookmark',{action:'remove',url:b.url});load()};
  const rw=el('div','row');rw.append(a,x);w.append(rw)});
 r.append(w);
}

/* ---------- загрузки ---------- */
function downloads(r){
 document.title='Загрузки';r.innerHTML='';
 const w=el('div','wrap');w.append(el('h1','','Загрузки'));
 if(!S.downloads.length)w.append(el('div','empty','Загрузок пока нет.'));
 S.downloads.slice().reverse().forEach(d=>{
  const rw=el('div','row');const t=el('div','tx');
  t.append(el('div','',d.name||d.url),el('div','d',d.url));rw.append(t);
  rw.append(el('span','state '+d.state,{running:'идёт…',done:'готово',failed:'ошибка'}[d.state]));
  if(d.state==='done'){
   const o=el('button','','Открыть');o.onclick=()=>cmd('dl_open',{id:d.id});
   const f=el('button','','В папке');f.onclick=()=>cmd('dl_folder',{id:d.id});rw.append(o,f)}
  w.append(rw)});
 if(S.downloads.length){const c=el('button','','Очистить список');c.onclick=async()=>{await cmd('dl_clear');load()};w.append(c)}
 r.append(w);
}

/* ---------- о программе ---------- */
function toast(t){let e=$('#toast');if(!e){e=el('div');e.id='toast';document.body.append(e)}e.textContent=t;e.classList.remove('on');void e.offsetWidth;e.classList.add('on');clearTimeout(e.__t);e.__t=setTimeout(()=>e.classList.remove('on'),1800)}
function about(r){
 document.title='О atmo';r.innerHTML='';
 const w=el('div','wrap');
 const hero=el('div','hero');hero.append(el('div','logo2','atmo'));
 const v=el('button','ver','Версия '+S.version);
 hero.append(v,el('div','d','Браузер, который сам выбирает, откуда выходить в сеть.'));w.append(hero);
 let n=0,t=0;
 v.onclick=ev=>{
  const now=Date.now();n=now-t<900?n+1:1;t=now;
  const rp=el('span','rip'),b=v.getBoundingClientRect();rp.style.left=(ev.clientX-b.left)+'px';rp.style.top=(ev.clientY-b.top)+'px';v.append(rp);setTimeout(()=>rp.remove(),650);
  v.classList.remove('hit');void v.offsetWidth;v.classList.add('hit');
  if(n>=3&&n<7){const k=7-n;toast('Ещё '+k+' '+(k===1?'нажатие':k<5?'нажатия':'нажатий')+'…')}
  if(n>=7){n=0;toast('Спускаемся…');document.body.classList.add('fade');setTimeout(()=>location.href='/play',700)}
 };
 const ua=navigator.userAgent,cm=(ua.match(/Edg\/([\d.]+)/)||ua.match(/Chrome\/([\d.]+)/)||[])[1]||'?';
 const sys=(navigator.userAgentData&&navigator.userAgentData.platform)||navigator.platform||'Windows';
 const rows=[['Движок страниц','WebView2 (Chromium '+cm+')'],['Система',sys],['Режим',S.settings.mode==='advanced'?'Изоляция сети':'Обычный браузер'],['Сетевой маршрут',S.route||'—'],['Закладок',String(S.bookmarks.length)],['Данные',S.dir||'—']];
 const card=el('div','box');rows.forEach(([k,val],i)=>{const rw=el('div','kv');rw.style.animationDelay=(i*50)+'ms';rw.append(el('span','k',k),el('span','val',val));card.append(rw)});
 w.append(card);
 const f=el('div','d','Страницы atmo отдаёт собственный локальный прокси, поэтому они работают, даже когда сеть закрыта.');f.style.marginTop='16px';w.append(f);
 r.append(w);
}

/* ---------- пасхалки ---------- */
const K='ArrowUp,ArrowUp,ArrowDown,ArrowDown,ArrowLeft,ArrowRight,ArrowLeft,ArrowRight,KeyB,KeyA'.split(',');
let ki=0;
addEventListener('keydown',e=>{ki=e.code===K[ki]?ki+1:(e.code===K[0]?1:0);if(ki===K.length){ki=0;document.body.classList.toggle('aurora')}});


load();
if(view==='downloads')setInterval(load,1500);
</script></body></html>"##;
