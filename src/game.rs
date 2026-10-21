//! Мини-игра «Шахта» (atmo://play): инкрементальная игра с физической киркой.

pub const GAME_HTML: &str = r##"<!doctype html>
<html lang="ru"><head><meta charset="utf-8"><meta name="color-scheme" content="dark"><title>Шахта</title>
<style>
:root{color-scheme:dark;--bg:#0d0e11;--panel:#17181c;--line:#2c2e35;--txt:#e8eaed;--mut:#9aa0a6;--acc:#5aa9ff;--gold:#f4ca4c}
*{box-sizing:border-box}
html,body{margin:0;height:100%;background:var(--bg);color:var(--txt);font:14px "Segoe UI",system-ui,sans-serif;overflow:hidden}
#app{display:flex;height:100%}
#stage{position:relative;flex:1;min-width:0}
canvas{display:block;width:100%;height:100%;touch-action:none;user-select:none}
#hud{position:absolute;left:14px;top:12px;display:flex;flex-direction:column;gap:6px;pointer-events:none}
#coinBox{display:flex;align-items:center;gap:8px;padding:7px 14px 7px 10px;border-radius:14px;background:rgba(20,21,25,.78);border:1px solid var(--line);backdrop-filter:blur(8px);transition:transform .18s cubic-bezier(.3,1.6,.5,1)}
#coinBox.pop{transform:scale(1.07)}
#coinIcon{width:22px;height:22px;flex:none}
#coins{font-size:22px;font-weight:700;color:var(--gold);font-variant-numeric:tabular-nums}
#depth{font-size:12px;color:var(--mut);padding-left:4px}
#oreList{display:flex;flex-direction:column;gap:3px;padding-left:4px;font-size:12px;color:#cfd2d8}
#oreList span{display:flex;align-items:center;gap:6px;animation:in .3s ease both}
#oreList i{width:9px;height:9px;border-radius:50%;display:inline-block}
#hint{position:absolute;left:50%;bottom:28px;transform:translateX(-50%);max-width:min(520px,86%);text-align:center;padding:12px 18px;border-radius:14px;background:rgba(20,21,25,.82);border:1px solid var(--line);color:#d7dae0;backdrop-filter:blur(8px);transition:opacity .6s,transform .6s;animation:in .6s .3s ease both}
#hint.off{opacity:0;transform:translateX(-50%) translateY(10px);pointer-events:none}
#toast{position:absolute;left:50%;top:16px;transform:translate(-50%,-14px);opacity:0;padding:9px 16px;border-radius:12px;background:var(--panel);border:1px solid var(--line);transition:.3s;pointer-events:none}
#toast.on{opacity:1;transform:translate(-50%,0)}
#back{position:absolute;right:14px;top:12px;padding:7px 12px;border-radius:12px;background:rgba(20,21,25,.78);border:1px solid var(--line);color:var(--mut);text-decoration:none;transition:.15s}
#back:hover{color:var(--txt);background:rgba(40,42,48,.9)}
#side{width:300px;flex:none;background:var(--panel);border-left:1px solid var(--line);display:flex;flex-direction:column;padding:14px 12px;gap:10px}
#side h2{margin:2px 4px 0;font-size:12px;letter-spacing:.09em;text-transform:uppercase;color:var(--mut)}
#ups{flex:1;overflow:auto;display:flex;flex-direction:column;gap:8px;padding:2px}
.up{text-align:left;font:inherit;color:inherit;background:#1e2025;border:1px solid var(--line);border-radius:12px;padding:10px 12px;cursor:pointer;display:flex;flex-direction:column;gap:3px;transition:transform .15s,border-color .15s,background .15s,opacity .2s}
.up:hover:not(.no){border-color:var(--acc);background:#20242c;transform:translateY(-1px)}
.up:active:not(.no){transform:scale(.985)}
.up .t{display:flex;justify-content:space-between;align-items:baseline}
.up .t i{font-style:normal;font-size:12px;color:var(--mut)}
.up small{color:var(--mut);font-size:12px}
.up .b{display:flex;justify-content:space-between;align-items:center;margin-top:3px}
.up em{font-style:normal;font-size:12px;color:#9fd0ff}
.up u{text-decoration:none;font-weight:700;color:var(--gold);font-variant-numeric:tabular-nums}
.up.no{opacity:.5;cursor:default}
.up.buy{animation:buy .5s ease}
@keyframes buy{0%{box-shadow:0 0 0 0 rgba(90,169,255,.8)}100%{box-shadow:0 0 0 14px rgba(90,169,255,0)}}
@keyframes in{from{opacity:0;transform:translateY(6px)}to{opacity:1;transform:none}}
#reset{font:inherit;color:var(--mut);background:transparent;border:1px solid var(--line);border-radius:10px;padding:7px;cursor:pointer;transition:.15s}
#reset:hover{color:#f2958f;border-color:#6a3a38}
</style></head>
<body><div id="app">
<div id="stage"><canvas id="c"></canvas>
 <div id="hud"><div id="coinBox"><svg id="coinIcon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="#f4ca4c"/><circle cx="12" cy="12" r="7" fill="none" stroke="#b8861f" stroke-width="1.6"/></svg><span id="coins">0</span></div><div id="depth">0 м</div><div id="oreList"></div></div>
 <div id="hint">Возьми кирку мышью за рукоять. Замахнись и ударь по породе: если просто прижать, ничего не отколется.</div>
 <div id="toast"></div><a id="back" href="/about">← назад</a>
</div>
<aside id="side"><h2>Улучшения</h2><div id="ups"></div><button id="reset">Сбросить прогресс</button></aside>
</div>
<script>
(function(){
'use strict';
var WH=520,CEIL=70,FLOOR=450,CW=640,XMIN=-330,UNITPX=26,GRAV=1800;
var $=function(i){return document.getElementById(i)};
var cv=$('c'),g=cv.getContext('2d');
var DPR=1,VW=900,VH=600,SC=1.15;
function clamp(v,a,b){return v<a?a:v>b?b:v}

/* ---------- шум и генератор ---------- */
function hash2(ix,iy,s){var h=(ix*374761393+iy*668265263+s*982451653)|0;h=(h^(h>>>13))*1274126177|0;return((h^(h>>>16))>>>0)/4294967296}
function vnoise(x,y,s){var ix=Math.floor(x),iy=Math.floor(y),fx=x-ix,fy=y-iy,u=fx*fx*(3-2*fx),v=fy*fy*(3-2*fy);
 var a=hash2(ix,iy,s),b=hash2(ix+1,iy,s),c=hash2(ix,iy+1,s),d=hash2(ix+1,iy+1,s);return a+(b-a)*u+(c-a)*v+(a-b-c+d)*u*v}
var seed=1;function rnd(){seed=(seed*1664525+1013904223)>>>0;return seed/4294967296}

/* ---------- руда ---------- */
var ORE=[null,
 {n:'Медь',v:1,c:[204,122,60],t:0,w:1},
 {n:'Железо',v:3,c:[142,158,186],t:1,w:.7},
 {n:'Серебро',v:9,c:[226,232,242],t:3,w:.45},
 {n:'Золото',v:30,c:[244,202,76],t:6,w:.3},
 {n:'Аметист',v:110,c:[184,108,255],t:10,w:.16}];

/* ---------- состояние и сохранение ---------- */
var S={coins:0,total:0,ore:[0,0,0,0,0,0],up:{pow:0,sharp:0,grip:0,magnet:0,hands:0,price:0,lamp:0},maxX:0,t:Date.now(),ema:4,digs:0};
var U=S.up,KEY='atmo.mine.v1',offlineMsg='';
function load(){
 try{var j=JSON.parse(localStorage.getItem(KEY)||'null');if(j){
  S.coins=+j.coins||0;S.total=+j.total||0;S.maxX=+j.maxX||0;S.ema=+j.ema||4;S.digs=+j.digs||0;S.t=+j.t||Date.now();
  if(j.ore&&j.ore.length===6)S.ore=j.ore;
  for(var k in S.up)S.up[k]=Math.max(0,Math.min(60,+((j.up||{})[k])||0));
  U=S.up;
 }}catch(e){}
}
function save(){try{S.t=Date.now();localStorage.setItem(KEY,JSON.stringify(S))}catch(e){}}

/* ---------- мир: чанки с маской породы ---------- */
var chunks={},startChunk=0,startX=0;
function hardAt(x){return Math.min(5,1+Math.max(0,x)/CW*.07)}
function priceMul(){return 1+.12*U.price}

function genChunk(i){
 var ch={i:i,x0:i*CW,mask:new Uint8Array(CW*WH),ore:new Uint8Array(CW*WH),cv:document.createElement('canvas'),spark:[]};
 ch.cv.width=CW;ch.cv.height=WH;ch.g=ch.cv.getContext('2d');
 var img=ch.g.createImageData(CW,WH),d=img.data;
 seed=(0x9e3779b9+i*7919)>>>0;
 var F=new Float32Array(CW*WH),MX=new Float32Array(CW*WH),T=new Uint8Array(CW*WH);
 var types=[],t,k,b;for(t=1;t<ORE.length;t++)if(i>=ORE[t].t)types.push(t);
 var nDep=Math.min(18,8+Math.floor(i*.7));
 function splat(bx,by,br,ty){
  var x1=Math.max(0,Math.floor(bx-br*3)),x2=Math.min(CW-1,Math.ceil(bx+br*3)),y1=Math.max(CEIL,Math.floor(by-br*3)),y2=Math.min(FLOOR-1,Math.ceil(by+br*3)),r2=br*br;
  for(var y=y1;y<=y2;y++)for(var x=x1;x<=x2;x++){var dx=x-bx,dy=y-by,cc=r2/(dx*dx+dy*dy+4),id=y*CW+x;F[id]+=cc;if(cc>MX[id]){MX[id]=cc;T[id]=ty}}
 }
 for(k=0;k<nDep;k++){
  var tw=0;types.forEach(function(t){tw+=ORE[t].w});var r=rnd()*tw,ty=types[0];
  for(var q=0;q<types.length;q++){r-=ORE[types[q]].w;if(r<=0){ty=types[q];break}}
  var cx=rnd()*CW,cy=CEIL+34+rnd()*(FLOOR-CEIL-68),R0=11+rnd()*(ty>=4?8:16),va=rnd()*Math.PI,nb=3+Math.floor(rnd()*5);
  if(i===0&&k<4){cx=70+k*85+rnd()*40;ty=1;R0=Math.max(R0,15)}
  for(b=0;b<nb;b++){
   var off=b===0?0:(rnd()-.35)*R0*2.6,bx=cx+Math.cos(va)*off+(rnd()-.5)*R0*.6,by=cy+Math.sin(va)*off+(rnd()-.5)*R0*.6,br=R0*(b===0?1:.35+rnd()*.5);
   splat(bx,by,br,ty);
  }
  ch.spark.push({x:cx,y:cy,r:R0,ph:rnd()*6.28});
 }
 for(var y=CEIL;y<FLOOR;y++)for(var x=0;x<CW;x++){
  var id=y*CW+x,wx=x+ch.x0;
  var n1=vnoise(wx*.021,y*.021,1),n2=vnoise(wx*.07,y*.07,2),n3=vnoise(wx*.31,y*.31,3);
  var bb=70+34*n1+16*n2+6*n3+7*Math.sin((y+n1*60)*.11)+(hash2(wx,y,21)-.5)*9;
  var cr=Math.abs(vnoise(wx*.017,y*.024,4)-.5);if(cr<.011)bb-=26*(1-cr/.011);
  var ed=Math.min(y-CEIL,FLOOR-1-y);if(ed<16)bb*=.72+.28*(ed/16);
  var R=bb*.93,G=bb*.97,B=bb*1.1;
  var f=F[id]+(vnoise(wx*.05,y*.05,5)-.5)*.5+(vnoise(wx*.13,y*.13,15)-.5)*.12;
  if(f>.9){
   var a=f>=1.02?1:(f-.94)/.08,rim=f<1.18?.78+.22*(f-1)/.18:1,oc=ORE[T[id]].c,sh=rim*(.78+.3*Math.min(1.4,Math.max(0,f-1)))+.14*(vnoise(wx*.035,y*.035,6)-.5)+(hash2(wx,y,22)-.5)*.14;
   R+=(oc[0]*sh-R)*a;G+=(oc[1]*sh-G)*a;B+=(oc[2]*sh-B)*a;
   if(f>1)ch.ore[id]=T[id];
  }
  var o=id*4;d[o]=R;d[o+1]=G;d[o+2]=B;d[o+3]=255;ch.mask[id]=1;
 }
 ch.g.putImageData(img,0,0);
 return ch;
}
function getChunk(i){
 if(i<0)return null;
 var ch=chunks[i];
 if(!ch){
  if(i<startChunk)ch={i:i,x0:i*CW,empty:true};else ch=genChunk(i);
  chunks[i]=ch;
  if(i===startChunk&&!ch.empty)faceCut(ch);
 }
 return ch;
}
function faceCut(ch){
 var pts=[[ch.x0-20,-20]],y;
 for(y=-20;y<=WH+20;y+=12)pts.push([startX+(vnoise(y*.03,0,9)-.5)*70+(vnoise(y*.2,1,9)-.5)*14,y]);
 pts.push([ch.x0-20,WH+20]);
 carve(pts,true);
}
function solid(x,y){
 if(y<CEIL||y>=FLOOR||x<0)return false;
 var ch=getChunk((x/CW)|0);if(ch.empty)return false;
 return ch.mask[(y|0)*CW+((x|0)-ch.x0)]===1;
}
function solidAny(x,y){return y<CEIL||y>=FLOOR||x<XMIN||solid(x,y)}

function pip(poly,x,y){var c=false;for(var i=0,j=poly.length-1;i<poly.length;j=i++){var a=poly[i],b=poly[j];if((a[1]>y)!==(b[1]>y)&&x<(b[0]-a[0])*(y-a[1])/(b[1]-a[1])+a[0])c=!c}return c}
function pathPoly(c,poly,cx,cy,k){c.beginPath();for(var i=0;i<poly.length;i++){var x=cx+(poly[i][0]-cx)*k,y=cy+(poly[i][1]-cy)*k;if(i)c.lineTo(x,y);else c.moveTo(x,y)}c.closePath()}
/* вынимает многоугольник из породы; возвращает сколько камня и руды убрано */
function carve(poly,quiet){
 var x1=1e9,x2=-1e9,y1=1e9,y2=-1e9,cx=0,cy=0,p;
 for(p=0;p<poly.length;p++){var q=poly[p];x1=Math.min(x1,q[0]);x2=Math.max(x2,q[0]);y1=Math.min(y1,q[1]);y2=Math.max(y2,q[1]);cx+=q[0];cy+=q[1]}
 cx/=poly.length;cy/=poly.length;
 y1=Math.max(CEIL,Math.floor(y1));y2=Math.min(FLOOR-1,Math.ceil(y2));
 var res={rock:0,ore:[0,0,0,0,0,0]},i1=Math.max(0,Math.floor(x1/CW)),i2=Math.floor(x2/CW);
 for(var i=i1;i<=i2;i++){
  var ch=getChunk(i);if(!ch||ch.empty)continue;
  var xa=Math.max(ch.x0,Math.floor(x1)),xb=Math.min(ch.x0+CW-1,Math.ceil(x2));
  for(var y=y1;y<=y2;y++)for(var x=xa;x<=xb;x++){
   var id=y*CW+(x-ch.x0);
   if(ch.mask[id]&&pip(poly,x+.5,y+.5)){ch.mask[id]=0;res.rock++;var o=ch.ore[id];if(o){res.ore[o]++;ch.ore[id]=0}}
  }
  var c=ch.g;c.save();c.translate(-ch.x0,0);
  c.globalCompositeOperation='source-atop';c.fillStyle='rgba(0,0,0,.42)';pathPoly(c,poly,cx,cy,1.24);c.fill();
  c.globalCompositeOperation='destination-out';c.fillStyle='#000';pathPoly(c,poly,cx,cy,1);c.fill();
  c.restore();
 }
 return res;
}
function crater(cx,cy,R,nx,ny){
 var n=10+((Math.random()*5)|0),pts=[],da=Math.atan2(-ny,-nx),a0=Math.random()*6.283;
 for(var k=0;k<n;k++){
  var a=a0+k/n*6.283+(Math.random()-.5)*.3,rad=R*(.68+.55*Math.random()),f=1+.4*Math.max(0,Math.cos(a-da));
  pts.push([cx-nx*R*.25+Math.cos(a)*rad*f,cy-ny*R*.25+Math.sin(a)*rad*f]);
 }
 return pts;
}

/* ---------- кирка: твёрдое тело ---------- */
var CP=[[64,-44,1],[64,46,1],[48,-26,1],[48,28,1],[43,0,1],[0,0,0],[-82,0,0]]; /* локальные точки контакта (центр масс в нуле) */
var pick={x:-150,y:FLOOR-8,a:.15,vx:0,vy:0,w:0,m:3.2,I:6500};
var grab=null,mouse={x:0,y:0},digCool=[0,0,0,0,0,0,0],T=0;
function segDist(px,py,ax,ay,bx,by){var dx=bx-ax,dy=by-ay,t=clamp(((px-ax)*dx+(py-ay)*dy)/(dx*dx+dy*dy),0,1),qx=ax+dx*t,qy=ay+dy*t;return[Math.hypot(px-qx,py-qy),qx,qy]}
function hitPick(wx,wy){
 var dx=wx-pick.x,dy=wy-pick.y,c=Math.cos(pick.a),s=Math.sin(pick.a),lx=c*dx+s*dy,ly=-s*dx+c*dy,best=null;
 var segs=[[-82,0,36,0],[64,-44,48,-26],[48,-26,43,0],[43,0,48,28],[48,28,64,46]];
 for(var i=0;i<segs.length;i++){var r=segDist(lx,ly,segs[i][0],segs[i][1],segs[i][2],segs[i][3]);if(r[0]<=16&&(!best||r[0]<best[0]))best=r}
 return best?[best[1],best[2]]:null;
}
function surfNormal(x,y){
 var rs=[3,6,11,20,34];
 for(var k=0;k<rs.length;k++){
  var sx=0,sy=0;
  for(var a=0;a<16;a++){var ca=Math.cos(a*.3927),sa=Math.sin(a*.3927);if(!solidAny(x+ca*rs[k],y+sa*rs[k])){sx+=ca;sy+=sa}}
  if(sx*sx+sy*sy>1e-6){var l=Math.hypot(sx,sy);return[sx/l,sy/l]}
 }
 return[-1,0];
}
function gripK(){return 650*(1+.08*U.grip)}
function step(dt){
 var c=Math.cos(pick.a),s=Math.sin(pick.a);
 if(grab){
  var rx=c*grab.lx-s*grab.ly,ry=s*grab.lx+c*grab.ly,px=pick.x+rx,py=pick.y+ry;
  var vx=pick.vx-pick.w*ry,vy=pick.vy+pick.w*rx,dx=mouse.x-px,dy=mouse.y-py,dl=Math.hypot(dx,dy);
  if(dl>180){dx*=180/dl;dy*=180/dl}
  var K=gripK(),C=2*Math.sqrt(K)*.78,ax=K*dx-C*vx,ay=K*dy-C*vy;
  pick.vx+=ax*dt;pick.vy+=ay*dt;pick.w+=(rx*ay-ry*ax)*pick.m/pick.I*dt;
 }
 pick.vy+=GRAV*dt;pick.w*=1-.5*dt;
 var sp=Math.hypot(pick.vx,pick.vy);if(sp>2600){pick.vx*=2600/sp;pick.vy*=2600/sp}
 pick.w=clamp(pick.w,-42,42);
 pick.x+=pick.vx*dt;pick.y+=pick.vy*dt;pick.a+=pick.w*dt;
 if(pick.a>Math.PI)pick.a-=6.2832;else if(pick.a<-Math.PI)pick.a+=6.2832;
 collide();
}
function collide(){
 var c=Math.cos(pick.a),s=Math.sin(pick.a);
 for(var j=0;j<CP.length;j++){
  var rx=c*CP[j][0]-s*CP[j][1],ry=s*CP[j][0]+c*CP[j][1],px=pick.x+rx,py=pick.y+ry,dig=CP[j][2];
  if(py>FLOOR)contact(j,rx,ry,0,-1,py-FLOOR,.25,false,px,py);
  if(py<CEIL)contact(j,rx,ry,0,1,CEIL-py,.25,false,px,py);
  if(px<XMIN)contact(j,rx,ry,1,0,XMIN-px,.2,false,px,py);
  if(solid(px,py)){
   var n=surfNormal(px,py),d=0;while(d<40&&solidAny(px+n[0]*d,py+n[1]*d))d++;
   contact(j,rx,ry,n[0],n[1],d,.1,dig,px,py);
  }
 }
}
function contact(j,rx,ry,nx,ny,depth,e,dig,px,py){
 var vx=pick.vx-pick.w*ry,vy=pick.vy+pick.w*rx,vn=vx*nx+vy*ny;
 if(dig&&vn<0)tryDig(j,px,py,-vn,nx,ny);
 if(vn<0){
  var rn=rx*ny-ry*nx,im=1/pick.m+rn*rn/pick.I,jn=-(1+e)*vn/im;
  pick.vx+=jn*nx/pick.m;pick.vy+=jn*ny/pick.m;pick.w+=(rx*jn*ny-ry*jn*nx)/pick.I;
  vx=pick.vx-pick.w*ry;vy=pick.vy+pick.w*rx;
  var tx=-ny,ty=nx,vt=vx*tx+vy*ty,rt=rx*ty-ry*tx,imt=1/pick.m+rt*rt/pick.I,jt=clamp(-vt/imt,-.6*jn,.6*jn);
  pick.vx+=jt*tx/pick.m;pick.vy+=jt*ty/pick.m;pick.w+=(rx*jt*ty-ry*jt*tx)/pick.I;
 }
 pick.x+=nx*depth*.5;pick.y+=ny*depth*.5;
}

/* ---------- удар, выемка, добыча ---------- */
var frags=[],parts=[],ghosts=[],floaters=[],acc=[0,0,0,0,0,0],shake=0,hudCoin={x:60,y:30},popT=0,hinted=false;
function tryDig(j,px,py,vin,nx,ny){
 if(T-digCool[j]<.08)return;
 var h=hardAt(px),vmin=380*Math.pow(h,.4)/(1+.07*U.sharp);
 if(vin<vmin)return;
 digCool[j]=T;
 var R=clamp((4+vin*.014)*(1+.16*U.pow)/Math.pow(h,.45),4,64);
 strike(px-nx*2,py-ny*2,R,nx,ny,vin,true);
}
function strike(wx,wy,R,nx,ny,vin,player){
 var res=carve(crater(wx,wy,R,nx,ny));
 if(res.rock<4)return 0;
 S.digs++;if(wx>S.maxX)S.maxX=wx;
 var gain=0,t;
 for(t=1;t<ORE.length;t++){
  if(!res.ore[t])continue;
  acc[t]+=res.ore[t];
  var u=Math.floor(acc[t]/UNITPX);
  if(u>0){acc[t]-=u*UNITPX;spawnFrags(t,u,wx,wy,nx,ny);gain+=u*ORE[t].v*priceMul()}
 }
 effects(wx,wy,R,nx,ny,vin,player,res.ore);
 if(!player)S.ema=S.ema*.92+gain*.08;
 if(!hinted){hinted=true;var hh=$('hint');if(hh)hh.classList.add('off')}
 return gain;
}
function spawnFrags(t,u,x,y,nx,ny){
 var n=Math.min(Math.ceil(u),8),per=u/n;
 for(var k=0;k<n;k++){var sp=120+Math.random()*320,an=Math.atan2(ny,nx)+(Math.random()-.5)*1.7;
  frags.push({t:t,u:per,x:x,y:y,vx:Math.cos(an)*sp,vy:Math.sin(an)*sp-120,rest:0,fly:-1,r:3+Math.min(5,Math.sqrt(per)*1.6),rot:Math.random()*6.28,sx:0,sy:0})}
}
function effects(x,y,R,nx,ny,vin,player,ore){
 var n=Math.min(18,4+(R/3|0)),k;
 for(k=0;k<n;k++){var an=Math.atan2(ny,nx)+(Math.random()-.5)*2.2,sp=80+Math.random()*(160+vin*.2);
  parts.push({k:'chip',x:x,y:y,vx:Math.cos(an)*sp,vy:Math.sin(an)*sp-80,l:0,ml:.7+Math.random()*.6,s:1.5+Math.random()*2.6,c:Math.random()<.5?'#6d727c':'#555a63'})}
 for(k=0;k<3;k++)parts.push({k:'dust',x:x+(Math.random()-.5)*R,y:y+(Math.random()-.5)*R,vx:nx*20+(Math.random()-.5)*30,vy:-15-Math.random()*20,l:0,ml:.8+Math.random()*.5,s:R*(.5+Math.random()*.4)});
 if(vin>700)for(k=0;k<6;k++){var a2=Math.random()*6.283,s2=200+Math.random()*400;parts.push({k:'spark',x:x,y:y,vx:Math.cos(a2)*s2,vy:Math.sin(a2)*s2,l:0,ml:.18+Math.random()*.15,s:0})}
 shake=Math.min(7,shake+vin/320);
 if(!player)ghosts.push({x:x-nx*50,y:y-ny*50,t:0});
}
function updateFrags(dt){
 var absorbT=Math.max(.25,1.1-.18*U.magnet),MR=U.magnet?60+40*U.magnet:0;
 for(var i=frags.length-1;i>=0;i--){
  var f=frags[i];
  if(f.fly>=0){
   f.fly+=dt/.55;var e=f.fly*f.fly*(3-2*f.fly);
   if(f.fly>=1){var v=f.u*ORE[f.t].v*priceMul();S.coins+=v;S.total+=v;S.ore[f.t]+=f.u;popT=.25;floaters.push({txt:'+'+fmt(v),x:hudCoin.x+16,y:hudCoin.y+18,t:0,s:1});frags.splice(i,1)}
   continue;
  }
  f.vy+=GRAV*.9*dt;
  var nx=f.x+f.vx*dt,ny=f.y+f.vy*dt;
  if(ny>FLOOR-f.r){ny=FLOOR-f.r;f.vy*=-.38;f.vx*=.7;if(Math.abs(f.vy)<45)f.vy=0}
  if(ny<CEIL+f.r){ny=CEIL+f.r;f.vy*=-.3}
  if(nx<XMIN){nx=XMIN;f.vx*=-.3}
  if(solid(nx,ny)){
   if(!solid(nx,f.y)){f.vy*=-.3;ny=f.y}else if(!solid(f.x,ny)){f.vx*=-.3;nx=f.x}else{nx=f.x;ny=f.y;f.vx=0;f.vy=0}
  }
  f.x=nx;f.y=ny;f.rot+=f.vx*dt*.03;
  if(Math.abs(f.vx)+Math.abs(f.vy)<40)f.rest+=dt;else f.rest=0;
  var dm=Math.hypot(pick.x-f.x,pick.y-f.y);
  if(MR&&dm<MR){f.vx+=(pick.x-f.x)/dm*2600*dt;f.vy+=(pick.y-f.y)/dm*2600*dt;f.rest=Math.max(f.rest,absorbT*.6)}
  if(f.rest>absorbT||(MR&&dm<26)){f.fly=0;f.sx=(f.x-cam)*SC;f.sy=f.y*SC}
 }
}
function updateParts(dt){
 var i;
 for(i=parts.length-1;i>=0;i--){var p=parts[i];p.l+=dt;if(p.l>=p.ml){parts.splice(i,1);continue}
  if(p.k==='chip'){p.vy+=GRAV*dt;}else if(p.k==='dust'){p.vx*=.96;p.vy*=.96}
  p.x+=p.vx*dt;p.y+=p.vy*dt;if(p.k==='chip'&&p.y>FLOOR-2){p.y=FLOOR-2;p.vy*=-.3;p.vx*=.6}}
 for(i=ghosts.length-1;i>=0;i--){ghosts[i].t+=dt;if(ghosts[i].t>.4)ghosts.splice(i,1)}
 for(i=floaters.length-1;i>=0;i--){floaters[i].t+=dt;if(floaters[i].t>1)floaters.splice(i,1)}
 shake*=Math.pow(.02,dt);popT=Math.max(0,popT-dt);
}
var autoT=0;
function updateAuto(dt){var rate=.2*U.hands;if(!rate)return;autoT+=dt*rate;var guard=0;while(autoT>=1&&guard++<6){autoT-=1;autoHit()}}
function autoHit(){
 var y=CEIL+14+Math.random()*(FLOOR-CEIL-28),x0=Math.max(0,Math.floor(cam)-120);
 for(var x=x0;x<x0+1600;x+=3){if(solid(x,y)){
  var R=clamp((4+420*.014)*(1+.16*U.pow)*.85/Math.pow(hardAt(x),.45),4,56);
  strike(x+3,y,R,-1,0,420,false);return}}
}

/* ---------- камера ---------- */
var cam=XMIN;
function updateCam(dt){
 var vw=VW/SC,target=cam;
 if(pick.x>cam+vw*.62)target=pick.x-vw*.62;else if(pick.x<cam+vw*.3)target=pick.x-vw*.3;
 target=Math.max(XMIN,target);cam+=(target-cam)*Math.min(1,dt*4);
}
var genBudget=0;
function ensureChunks(){
 var i1=Math.max(0,Math.floor((cam-50)/CW)),i2=Math.floor((cam+VW/SC+CW*.6)/CW);
 for(var i=i1;i<=i2;i++)if(!chunks[i]){getChunk(i);return}
}

/* ---------- отрисовка ---------- */
function rrect(c,x,y,w,h,r){c.beginPath();c.moveTo(x+r,y);c.lineTo(x+w-r,y);c.quadraticCurveTo(x+w,y,x+w,y+r);c.lineTo(x+w,y+h-r);c.quadraticCurveTo(x+w,y+h,x+w-r,y+h);c.lineTo(x+r,y+h);c.quadraticCurveTo(x,y+h,x,y+h-r);c.lineTo(x,y+r);c.quadraticCurveTo(x,y,x+r,y);c.closePath()}
var bgCv=null;
function makeBg(){
 bgCv=document.createElement('canvas');bgCv.width=512;bgCv.height=WH;var c=bgCv.getContext('2d'),img=c.createImageData(512,WH),d=img.data;
 for(var y=0;y<WH;y++)for(var x=0;x<512;x++){
  var n=vnoise(x*.02,y*.02,11)*.6+vnoise(x*.09,y*.09,12)*.4,b=24+20*n,o=(y*512+x)*4;
  var wrap=Math.min(x,511-x)<30?.9:1;
  d[o]=b*.9*wrap;d[o+1]=b*.95*wrap;d[o+2]=b*1.1*wrap;d[o+3]=255;
 }
 c.putImageData(img,0,0);
}
function sx(x){return(x-cam)*SC}
function drawSlab(top){
 var y0=top?0:FLOOR,y1=top?CEIL:WH;
 var gr=g.createLinearGradient(0,y0*SC,0,y1*SC);
 if(top){gr.addColorStop(0,'#1d1f24');gr.addColorStop(1,'#3b3e46')}else{gr.addColorStop(0,'#3b3e46');gr.addColorStop(1,'#1c1e23')}
 g.fillStyle=gr;g.fillRect(0,y0*SC,VW,(y1-y0)*SC);
 g.strokeStyle='rgba(255,255,255,.10)';g.lineWidth=1.5;g.beginPath();var ey=(top?CEIL:FLOOR)*SC;g.moveTo(0,ey);g.lineTo(VW,ey);g.stroke();
 g.strokeStyle='rgba(0,0,0,.35)';g.lineWidth=1;g.beginPath();
 var x0=Math.floor(cam/230)*230;
 for(var x=x0;x<cam+VW/SC+230;x+=230){var jx=x+hash2(Math.floor(x/230),7,3)*60,xx=sx(jx);g.moveTo(xx,y0*SC);g.lineTo(xx+8,y1*SC)}
 g.stroke();
}
function drawPickShape(c,alpha){
 c.globalAlpha=alpha;c.save();c.translate(-20,0);
 var gr=c.createLinearGradient(0,-6,0,6);gr.addColorStop(0,'#b87f49');gr.addColorStop(.5,'#8a5a2e');gr.addColorStop(1,'#5e3b1c');
 c.fillStyle=gr;rrect(c,-62,-5,118,10,5);c.fill();
 c.strokeStyle='rgba(40,22,8,.35)';c.lineWidth=1;c.beginPath();c.moveTo(-56,-1.5);c.lineTo(40,-2);c.moveTo(-50,2);c.lineTo(44,1.6);c.stroke();
 c.fillStyle='#2b2d33';rrect(c,-62,-6,18,12,4);c.fill();
 var hg=c.createLinearGradient(44,-46,86,46);hg.addColorStop(0,'#e1e6ee');hg.addColorStop(.5,'#8f98a8');hg.addColorStop(1,'#565d6a');
 c.fillStyle=hg;c.beginPath();c.moveTo(84,-44);c.quadraticCurveTo(60,-24,62,-8);c.lineTo(62,9);c.quadraticCurveTo(60,26,84,46);c.quadraticCurveTo(70,30,48,10);c.lineTo(48,-9);c.quadraticCurveTo(70,-28,84,-44);c.closePath();c.fill();
 c.strokeStyle='rgba(20,22,28,.75)';c.lineWidth=1.2;c.stroke();
 c.restore();c.globalAlpha=1;
}
function fmt(n){if(n<1000)return n<10&&n%1?n.toFixed(1):String(Math.floor(n));var u=['K','M','B','T'],i=-1;while(n>=1000&&i<3){n/=1000;i++}return(n<10?n.toFixed(2):n<100?n.toFixed(1):Math.floor(n))+u[i]}
function draw(){
 g.setTransform(DPR,0,0,DPR,0,0);
 var shx=(Math.random()-.5)*shake,shy=(Math.random()-.5)*shake;
 g.fillStyle='#0b0c0f';g.fillRect(0,0,VW,VH);
 g.save();g.translate(shx,shy);
 /* задняя стена */
 var bw=512*SC,bo=-((cam*.85*SC)%bw);
 for(var bx=bo-bw;bx<VW+bw;bx+=bw)g.drawImage(bgCv,bx,0,bw,WH*SC);
 /* свет снаружи слева */
 var ex=sx(XMIN-30),eg=g.createRadialGradient(ex,260*SC,10,ex,260*SC,520*SC);eg.addColorStop(0,'rgba(255,238,200,.55)');eg.addColorStop(1,'rgba(255,238,200,0)');
 g.fillStyle=eg;g.fillRect(0,0,VW,WH*SC);
 /* порода */
 var i1=Math.max(0,Math.floor(cam/CW)),i2=Math.floor((cam+VW/SC)/CW),i,ch;
 for(i=i1;i<=i2;i++){ch=chunks[i];if(ch&&!ch.empty)g.drawImage(ch.cv,sx(ch.x0),0,CW*SC,WH*SC)}
 /* блеск руды */
 for(i=i1;i<=i2;i++){ch=chunks[i];if(!ch||ch.empty)continue;
  for(var k=0;k<ch.spark.length;k++){var sp=ch.spark[k],ph=Math.sin(T*2.2+sp.ph),al=ph>.8?(ph-.8)*5:0;if(al<=0)continue;
   var qx=ch.x0+sp.x+Math.cos(sp.ph*7)*sp.r*.5,qy=sp.y+Math.sin(sp.ph*5)*sp.r*.5;
   if(ch.ore[(qy|0)*CW+(qx-ch.x0|0)]){var X=sx(qx),Y=qy*SC;g.strokeStyle='rgba(255,255,255,'+al.toFixed(2)+')';g.lineWidth=1.3;g.beginPath();g.moveTo(X-4,Y);g.lineTo(X+4,Y);g.moveTo(X,Y-4);g.lineTo(X,Y+4);g.stroke()}}}
 drawSlab(true);drawSlab(false);
 /* осколки */
 for(i=0;i<frags.length;i++){var f=frags[i],oc=ORE[f.t].c,fx,fy;
  if(f.fly>=0){var e=f.fly*f.fly*(3-2*f.fly);fx=f.sx+(hudCoin.x-f.sx)*e;fy=f.sy+(hudCoin.y-f.sy)*e-Math.sin(e*3.14)*40}else{fx=sx(f.x);fy=f.y*SC}
  g.save();g.translate(fx,fy);g.rotate(f.rot);g.fillStyle='rgb('+oc[0]+','+oc[1]+','+oc[2]+')';g.strokeStyle='rgba(0,0,0,.45)';g.lineWidth=1;
  var rr=f.r*SC;g.beginPath();g.moveTo(-rr,0);g.lineTo(-rr*.3,-rr);g.lineTo(rr*.9,-rr*.4);g.lineTo(rr,rr*.5);g.lineTo(-rr*.2,rr);g.closePath();g.fill();g.stroke();g.restore()}
 /* призраки подмастерьев */
 for(i=0;i<ghosts.length;i++){var gh=ghosts[i],tt=gh.t/.4;g.save();g.translate(sx(gh.x),gh.y*SC);g.rotate(-1.1+tt*1.3);g.scale(SC*.7,SC*.7);drawPickShape(g,.55*(1-tt));g.restore()}
 /* кирка */
 g.save();g.translate(sx(pick.x),pick.y*SC);g.rotate(pick.a);g.scale(SC,SC);
 if(grab){g.fillStyle='rgba(255,230,160,.9)';g.beginPath();g.arc(grab.lx,grab.ly,4,0,6.283);g.fill()}
 drawPickShape(g,1);g.restore();
 /* частицы */
 for(i=0;i<parts.length;i++){var p=parts[i],q=p.l/p.ml;
  if(p.k==='dust'){g.fillStyle='rgba(150,150,158,'+(.28*(1-q)).toFixed(3)+')';g.beginPath();g.arc(sx(p.x),p.y*SC,p.s*SC*(.6+q),0,6.283);g.fill()}
  else if(p.k==='chip'){g.globalAlpha=1-q;g.fillStyle=p.c;g.fillRect(sx(p.x),p.y*SC,p.s*SC,p.s*SC);g.globalAlpha=1}
  else{g.strokeStyle='rgba(255,220,140,'+(1-q).toFixed(2)+')';g.lineWidth=1.5;g.beginPath();g.moveTo(sx(p.x),p.y*SC);g.lineTo(sx(p.x-p.vx*.03),(p.y-p.vy*.03)*SC);g.stroke()}}
 g.restore();
 /* темнота и свет лампы */
 var lx=sx(pick.x),ly=pick.y*SC,lr=(240+60*U.lamp)*SC,dg=g.createRadialGradient(lx,ly,lr*.12,lx,ly,lr);
 dg.addColorStop(0,'rgba(7,8,11,0)');dg.addColorStop(1,'rgba(7,8,11,.74)');g.fillStyle=dg;g.fillRect(0,0,VW,VH);
 var wg=g.createRadialGradient(lx,ly,0,lx,ly,lr*.5);wg.addColorStop(0,'rgba(255,196,110,.10)');wg.addColorStop(1,'rgba(255,196,110,0)');
 g.globalCompositeOperation='lighter';g.fillStyle=wg;g.fillRect(0,0,VW,VH);g.globalCompositeOperation='source-over';
 /* всплывающие числа */
 g.font='700 15px Segoe UI, system-ui, sans-serif';g.textAlign='left';
 for(i=0;i<floaters.length;i++){var fl=floaters[i];g.fillStyle='rgba(255,222,120,'+(1-fl.t).toFixed(2)+')';g.fillText(fl.txt,fl.x,fl.y-fl.t*26)}
}

/* ---------- интерфейс ---------- */
var UP=[
 {k:'pow',n:'Головка кирки',d:'Крупнее выемки при ударе',b:12,g:1.55,s:function(l){return'размер +'+16*l+'%'}},
 {k:'sharp',n:'Заточка',d:'Работают и более лёгкие удары',b:18,g:1.6,s:function(l){return'порог −'+Math.round((1-1/(1+.07*l))*100)+'%'}},
 {k:'grip',n:'Хват',d:'Кирка послушнее, замах точнее',b:20,g:1.6,s:function(l){return'жёсткость +'+8*l+'%'}},
 {k:'magnet',n:'Магнит',d:'Осколки сами летят к кирке',b:35,g:1.75,s:function(l){return l?'радиус '+(60+40*l):'выкл'}},
 {k:'hands',n:'Подмастерья',d:'Сами бьют по забою, пока ты занят',b:45,g:1.7,s:function(l){return(.2*l).toFixed(1)+' ударов/с'}},
 {k:'price',n:'Торговец',d:'Руда продаётся дороже',b:30,g:1.8,s:function(l){return'цена +'+12*l+'%'}},
 {k:'lamp',n:'Лампа',d:'Свет дальше, руда видна',b:25,g:1.85,s:function(l){return'радиус '+(240+60*l)}}
];
function cost(u){return Math.ceil(u.b*Math.pow(u.g,U[u.k]))}
var upEls={};
function buildPanel(){
 var box=$('ups');if(!box)return;box.innerHTML='';
 UP.forEach(function(u){
  var b=document.createElement('button');b.className='up';
  b.innerHTML='<span class="t"><b></b><i></i></span><small></small><span class="b"><em></em><u></u></span>';
  b.onclick=function(){var c=cost(u);if(S.coins<c)return;S.coins-=c;U[u.k]++;b.classList.remove('buy');void b.offsetWidth;b.classList.add('buy');refresh();save()};
  box.appendChild(b);upEls[u.k]=b;
 });
 refresh();
}
function refresh(){
 UP.forEach(function(u){var b=upEls[u.k];if(!b)return;var c=cost(u);
  b.querySelector('b').textContent=u.n;b.querySelector('i').textContent='ур. '+U[u.k];
  b.querySelector('small').textContent=u.d;b.querySelector('em').textContent=u.s(U[u.k]);b.querySelector('u').textContent=fmt(c);
  b.classList.toggle('no',S.coins<c)});
}
var lastHud=0;
function hud(){
 var el=$('coins');if(el){el.textContent=fmt(S.coins);el.parentNode.classList.toggle('pop',popT>0)}
 var d=$('depth');if(d)d.textContent=Math.floor(Math.max(0,pick.x)/40)+' м';
 var o=$('oreList');if(o){var s='';for(var t=1;t<ORE.length;t++)if(S.ore[t]>0)s+='<span><i style="background:rgb('+ORE[t].c+')"></i>'+ORE[t].n+' '+fmt(S.ore[t])+'</span>';if(o.__s!==s){o.__s=s;o.innerHTML=s}}
}
function toast(t){var e=$('toast');if(!e)return;e.textContent=t;e.classList.add('on');setTimeout(function(){e.classList.remove('on')},4200)}

/* ---------- ввод ---------- */
function toWorld(e){var r=cv.getBoundingClientRect();mouse.x=(e.clientX-r.left)/SC+cam;mouse.y=(e.clientY-r.top)/SC}
cv.addEventListener('pointerdown',function(e){
 toWorld(e);var a=hitPick(mouse.x,mouse.y);
 if(a){grab={lx:a[0],ly:a[1]};try{cv.setPointerCapture(e.pointerId)}catch(_){}cv.style.cursor='grabbing'}
});
cv.addEventListener('pointermove',function(e){toWorld(e);if(!grab)cv.style.cursor=hitPick(mouse.x,mouse.y)?'grab':'default'});
function release(){grab=null;cv.style.cursor='default'}
cv.addEventListener('pointerup',release);cv.addEventListener('pointercancel',release);cv.addEventListener('lostpointercapture',release);
cv.addEventListener('contextmenu',function(e){e.preventDefault()});
$('reset')&&($('reset').onclick=function(){if(confirm('Сбросить весь прогресс шахты?')){try{localStorage.removeItem(KEY)}catch(_){}location.reload()}});

/* ---------- цикл ---------- */
var last=0,hudT=0,saveT=0;
function frame(ts){
 if(!last)last=ts;var dt=Math.min(.05,Math.max(0,(ts-last)/1000));last=ts;T+=dt;
 try{
  var n=Math.max(1,Math.ceil(dt/(1/240)));for(var k=0;k<n;k++)step(dt/n);
  updateAuto(dt);updateFrags(dt);updateParts(dt);updateCam(dt);ensureChunks();draw();
  hudT+=dt;if(hudT>.08){hudT=0;hud();if(popT<=0)refresh()}
  saveT+=dt;if(saveT>6){saveT=0;save()}
 }catch(err){if(window.__TEST)throw err}
 requestAnimationFrame(frame);
}
function fit(){
 var r=cv.getBoundingClientRect();DPR=Math.min(2,window.devicePixelRatio||1);
 VW=Math.max(300,r.width);VH=Math.max(240,r.height);cv.width=Math.round(VW*DPR);cv.height=Math.round(VH*DPR);SC=VH/WH;
 var c=$('coinIcon');if(c){var q=c.getBoundingClientRect(),cr=cv.getBoundingClientRect();hudCoin.x=q.left-cr.left+q.width/2;hudCoin.y=q.top-cr.top+q.height/2}
}
window.addEventListener('resize',fit);
window.addEventListener('beforeunload',save);

/* ---------- старт ---------- */
load();
var away=Math.min(8*3600e3,Math.max(0,Date.now()-S.t));
if(U.hands>0&&away>60e3){var gain=Math.floor(away/1000*.2*U.hands*Math.max(1,S.ema)*.5);if(gain>0){S.coins+=gain;S.total+=gain;offlineMsg='Пока тебя не было, подмастерья принесли '+fmt(gain)+' монет'}}
startX=Math.max(0,S.maxX-140);startChunk=Math.floor(startX/CW);
pick.x=Math.max(-150,startX-170);cam=Math.max(XMIN,pick.x-VW/SC*.3);
makeBg();fit();buildPanel();getChunk(startChunk);
if(offlineMsg)toast(offlineMsg);
if(S.digs>0){hinted=true;var hh=$('hint');if(hh)hh.classList.add('off')}
if(window.__TEST)window.__mine={pick:pick,mouse:mouse,S:S,U:U,frame:frame,step:step,chunks:chunks,carve:carve,solid:solid,strike:strike,frags:frags,get grab(){return grab},get cam(){return cam},hitPick:hitPick,CP:CP,FLOOR:FLOOR,CEIL:CEIL,CW:CW,ORE:ORE,hardAt:hardAt,autoHit:autoHit,setU:function(k,v){U[k]=v}};
requestAnimationFrame(frame);
})();

</script></body></html>"##;
