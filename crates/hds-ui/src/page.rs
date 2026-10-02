//! Встроенная страница Rust-UI (одна HTML, без внешних ресурсов). Перепроектирована
//! под Rust-стек: статус (индекс + роли `llm-host`), поиск, RAG-вопрос, управление
//! индексацией. Python-`assets/ui.html` остаётся для Python-версии.

/// HTML-страница (`text/html; charset=utf-8`).
pub const PAGE: &str = r#"<!doctype html>
<html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Hermes Disk Search</title>
<style>
 body{font:14px/1.5 system-ui,Segoe UI,Arial;margin:0;background:#0f1720;color:#e6edf3}
 header{padding:14px 20px;background:#111c28;border-bottom:1px solid #24384f}
 h1{font-size:18px;margin:0}
 main{max-width:900px;margin:0 auto;padding:16px 20px}
 section{background:#131f2b;border:1px solid #24384f;border-radius:10px;padding:14px 16px;margin:14px 0}
 h2{font-size:15px;margin:0 0 10px;color:#9fd0ff}
 input,button,textarea{font:inherit;padding:8px 10px;border-radius:8px;border:1px solid #2a3a55;background:#0d1620;color:#e6edf3}
 input{min-width:280px}button{cursor:pointer;background:#1c2f44}
 textarea{width:100%;min-height:64px;box-sizing:border-box}
 button:hover{background:#25405e}
 .row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
 pre{white-space:pre-wrap;word-break:break-word;background:#0d1620;border:1px solid #24384f;border-radius:8px;padding:10px;margin:10px 0 0}
 .muted{color:#8aa0b6}.ok{color:#6ee7a8}.err{color:#ff9a9a}
 .res{border-top:1px solid #24384f;padding:8px 0}
 .loc{color:#9fd0ff;word-break:break-all}
</style></head><body>
<header><h1>Hermes Disk Search — Rust UI</h1></header>
<main>
 <section><h2>Состояние</h2><div id="status" class="muted">…</div></section>

 <section><h2>Проверка компонентов</h2>
  <div class="row"><button onclick="loadDiag()">Обновить</button>
   <span id="dmsg" class="muted"></span></div>
  <div id="diag" class="muted">…</div></section>

 <section><h2>Поиск</h2>
  <div class="row"><input id="q" placeholder="например: накладная склад">
   <input id="lim" type="number" value="8" min="1" max="30" style="min-width:70px">
   <button onclick="doSearch()">Найти</button></div>
  <div id="sres"></div></section>

 <section><h2>Вопрос (ответ по файлам)</h2>
  <div class="row"><input id="aq" placeholder="о чём … ?" style="min-width:360px">
   <button onclick="doAsk()">Спросить</button></div>
  <pre id="aans" class="muted" style="display:none"></pre></section>

 <section><h2>Дерево индекса</h2>
  <div class="row"><button onclick="loadTree()">Показать/обновить</button>
   <span id="tmsg" class="muted"></span></div>
  <div id="tree" class="muted">…</div></section>

 <section><h2>Настройки (config.yaml)</h2>
  <div class="muted">Корни индексации (по одному на строку):</div>
  <textarea id="roots" rowspan="3" spellcheck="false"></textarea>
  <div class="row"><button onclick="saveRoots()">Сохранить корни</button>
   <span id="cmsg" class="muted"></span></div>
  <div class="muted">Исключённые пути (по одному на строку):</div>
  <textarea id="excl" spellcheck="false"></textarea>
  <div class="row"><button onclick="saveExcl()">Сохранить исключения</button></div>
 </section>

 <section><h2>Индексация</h2>
  <div class="row">
   <button onclick="act('/api/index/start')">Старт</button>
   <button onclick="act('/api/index/start?full=1')">Полная</button>
   <button onclick="act('/api/index/stop')">Стоп</button>
   <button onclick="act('/api/index/pause')">Пауза</button>
   <button onclick="act('/api/index/resume')">Продолжить</button>
   <span id="imsg" class="muted"></span></div></section>
</main>
<script>
async function jget(u){ const r=await fetch(u); return await r.json(); }
async function jpost(u,body){ const r=await fetch(u,{method:'POST',headers:{'Content-Type':'application/json','X-HDS-UI':'1'},body:JSON.stringify(body||{})}); return await r.json(); }
function esc(s){ return (s||'').replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c])); }
async function refresh(){
  try{ const s=await jget('/api/status');
    const idx=s.index||{}, lh=s.llm_host||{};
    let h='Файлов: сточ. <b>'+(idx.chunks??'?')+'</b> · статусы: '+esc(idx.statuses||'-')+
          ' · по типам: '+esc(idx.kinds||'-')+'\n';
    h+='Индексация: '+(idx.running?'<span class="ok">идёт</span>':'не запущена')+
       (idx.paused? ' · <span class="err">пауза</span>':'')+'\n';
    h+='llm-host: '+(lh.up? '<span class="ok">up</span> ('+esc(lh.mode||'')+', pid '+(lh.pid??'?')+')'
                          : '<span class="err">не отвечает</span>')+'\n';
    if(lh.roles) h+='роли: '+esc(lh.roles)+'\n';
    document.getElementById('status').innerHTML='<pre>'+h+'</pre>';
  }catch(e){ document.getElementById('status').textContent='ошибка статуса: '+e; }
}
async function doSearch(){
  const q=document.getElementById('q').value.trim(); if(!q) return;
  const lim=document.getElementById('lim').value||8;
  const box=document.getElementById('sres'); box.innerHTML='<div class="muted">…</div>';
  try{ const res=await jget('/api/search?q='+encodeURIComponent(q)+'&limit='+lim);
    if(!res.length){ box.innerHTML='<div class="muted">Ничего не найдено.</div>'; return; }
    box.innerHTML=res.map((r,i)=>'<div class="res"><b>['+(i+1)+']</b> <span class="loc">'+
      esc(r.location)+'</span> <span class="muted">(score '+r.score+')</span><div>'+
      esc(r.snippet)+'</div></div>').join('');
  }catch(e){ box.innerHTML='<div class="err">ошибка: '+esc(''+e)+'</div>'; }
}
async function doAsk(){
  const q=document.getElementById('aq').value.trim(); if(!q) return;
  const p=document.getElementById('aans'); p.style.display='block'; p.textContent='…';
  try{ const r=await jget('/api/ask?q='+encodeURIComponent(q));
    let t=r.answer||''; (r.sources||[]).forEach((s,i)=>{ t+='\n['+(i+1)+'] '+s.location; });
    p.textContent=t;
  }catch(e){ p.textContent='ошибка: '+e; }
}
async function act(u){
  const m=document.getElementById('imsg'); m.textContent='…';
  try{ const r=await jpost(u); m.textContent=r.msg||'ok'; }catch(e){ m.textContent='ошибка: '+e; }
  refresh();
}
function stIcon(s){ return s==='done'?'&#9989;':(s==='partial'?'&#9888;&#65039;':'&#11036;'); }
function nodeHtml(n,depth){
  const pad='&nbsp;&nbsp;'.repeat(depth);
  let h=pad+stIcon(n.status)+' <b>'+esc(n.name)+'</b> <span class="muted">(файлов '+n.files+', инд '+n.indexed+')</span><br>';
  (n.children||[]).slice(0,60).forEach(c=>{ h+=nodeHtml(c,depth+1); });
  return h;
}
async function loadTree(){
  const t=document.getElementById('tree'); t.textContent='…';
  try{ const j=await jget('/api/tree');
    document.getElementById('tmsg').textContent='папок: '+(j.dirs??'?')+' · файлов на диске: '+(j.disk_files??'?');
    t.innerHTML=(j.trees||[]).map(n=>nodeHtml(n,0)).join('')||'<span class="muted">нет данных</span>';
  }catch(e){ t.textContent='ошибка: '+e; }
}
async function loadDiag(){
  const d=document.getElementById('diag'); d.textContent='…';
  try{ const j=await jget('/api/diagnostics');
    d.innerHTML=(j.checks||[]).map(c=>'<div>'+stIcon(c.status==='ok'?'done':(c.status==='warn'?'partial':'none'))+
      ' <b>'+esc(c.title||c.id)+'</b>: '+esc(c.msg||'')+'</div>').join('');
  }catch(e){ d.textContent='ошибка: '+e; }
}
async function loadCfg(){
  try{ const c=await jget('/api/config');
    document.getElementById('roots').value=(c.roots||[]).join('\n');
    document.getElementById('excl').value=(c.exclude_paths||[]).join('\n');
  }catch(e){}
}
function linesOf(id){ return document.getElementById(id).value.split('\n').map(s=>s.trim()).filter(Boolean); }
async function saveRoots(){ const m=document.getElementById('cmsg'); m.textContent='…';
  try{ const r=await jpost('/api/roots/save',{roots:linesOf('roots')}); m.textContent=r.msg||'ok'; }catch(e){ m.textContent='ошибка: '+e; } }
async function saveExcl(){ const m=document.getElementById('cmsg'); m.textContent='…';
  try{ const r=await jpost('/api/config/excludes',{paths:linesOf('excl')}); m.textContent=r.msg||'ok'; }catch(e){ m.textContent='ошибка: '+e; } }
refresh(); setInterval(refresh, 3000); loadDiag(); loadTree(); loadCfg();
</script></body></html>
"#;
