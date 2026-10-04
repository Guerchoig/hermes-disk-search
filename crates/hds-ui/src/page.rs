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
 .warnbox{color:#ffd479;margin-top:8px;line-height:1.4}
 nav.tabs{display:flex;gap:6px;margin:0 0 12px}
 nav.tabs button{border-radius:8px;background:#0d1620;color:#8aa0b6}
 nav.tabs button.active{background:#1c2f44;color:#9fd0ff;border-color:#3a5a80}
 table.tr{width:100%;border-collapse:collapse}
 table.tr th,table.tr td{border-bottom:1px solid #24384f;padding:6px 8px;text-align:left;vertical-align:top}
 table.tr tbody tr{cursor:pointer}
 table.tr tbody tr:hover{background:#17273a}
 table.tr tbody tr.sel{background:#1c2f44}
 #trprev{max-height:340px;overflow:auto}
  #tree details{margin:2px 0}
  #tree details details{margin-left:20px;border-left:1px solid #24384f;padding-left:8px}
  summary{cursor:pointer;user-select:none;list-style-position:outside}
  summary::-webkit-details-marker{color:#8aa0b6}
 .modal{position:fixed;inset:0;background:rgba(3,8,14,.72);display:none;align-items:center;justify-content:center;z-index:9}
 .modal.open{display:flex}
 .modal .box{background:#131f2b;border:1px solid #24384f;border-radius:10px;padding:16px;min-width:420px;max-width:720px;max-height:82vh;overflow:auto}
 .modal .box h3{margin:0 0 10px;font-size:15px;color:#9fd0ff}
 .modal input{min-width:200px}
 .modal .box table input{width:100%}
</style></head><body>
<header><h1>Hermes Disk Search — Rust UI</h1></header>
<main>
 <nav class="tabs">
  <button data-tab="search" class="active" onclick="showTab('search')">Поиск</button>
  <button data-tab="transcribe" onclick="showTab('transcribe')">Транскрибация</button>
 </nav>
 <div id="tab-search">
 <section><h2>Состояние</h2><div id="status" class="muted">…</div></section>

 <section><h2>Резидент llm-host</h2>
  <div class="row"><button onclick="restartLlmHost()">Перезапустить llm-host</button>
   <span id="lhmsg" class="muted"></span></div>
  <div class="muted">Останавливает и поднимает заново резидент (порты 8010–8012, владелец GPU). Модель перезагрузится — первый ответ будет с задержкой.</div></section>

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
   <button onclick="treeAll(true)">Развернуть всё</button>
   <button onclick="treeAll(false)">Свернуть всё</button>
   <span id="tmsg" class="muted"></span></div>
  <div class="muted">Клик по папке — свернуть/развернуть. 🟢 индекс актуален · 🟡 есть не проиндексированное · ⬜ не индексировано.</div>
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

 <section><h2>Настройки Cline</h2>
  <div class="row"><button onclick="clineSync()">Синхронизировать настройки Cline</button>
   <span id="clmsg" class="muted"></span></div>
  <div class="muted">Приводит Cline к этому конфигу: окна контекста моделей = слоты llm-host
   (<code>ctx_per_slot</code>), MCP-сервер disk-search, правило и скилл. Установка/перезапуск
   Cline не нужны, но после правки моделей или MCP Cline надо перезапустить — об этом будет
   предупреждение. Правило подхватится в новой сессии.</div>
  <div id="clres" class="muted"></div></section>

 <section><h2>Индексация</h2>
  <div class="row">
   <button onclick="act('/api/index/start')">Старт</button>
   <button onclick="act('/api/index/start?full=1')">Полная</button>
   <button onclick="act('/api/index/stop')">Стоп</button>
   <button onclick="act('/api/index/pause')">Пауза</button>
   <button onclick="act('/api/index/resume')">Продолжить</button>
   <span id="imsg" class="muted"></span></div></section>

  <section><h2>Демон индексации</h2>
   <div class="row"><span id="wdaemon" class="muted">…</span></div>
   <div class="row" style="margin-top:8px">
    <button onclick="watchAct('start')">Запустить</button>
    <button onclick="watchAct('stop')">Остановить</button>
    <button onclick="watchAct('restart')">Перезапустить</button>
    <button onclick="loadWatchDaemon()">Обновить состояние</button>
    <span id="wdaemonmsg" class="muted"></span></div>
   <div class="muted">Это отдельный процесс <code>hds watch</code> (его держит файл
    <code>watch.lock</code>), с <code>llm-host</code> он не связан: остановка — мягкая через
    <code>index.stop</code>, запуск — без окна. Кнопки «Старт/Стоп/Пауза» выше управляют
    индексацией внутри UI и сигналами <code>index.pause</code>/<code>index.stop</code>.</div></section>
 </div>

 <div id="tab-transcribe" hidden>
  <section><h2>Демон автотранскрибации</h2>
   <div class="row"><span id="trdaemon" class="muted">…</span></div>
   <div class="row" style="margin-top:8px">
    <button onclick="daemonAct('start')">Запустить</button>
    <button onclick="daemonAct('stop')">Остановить</button>
    <button onclick="daemonAct('restart')">Перезапустить</button>
    <button onclick="loadDaemon()">Обновить состояние</button>
    <span id="trdaemonmsg" class="muted"></span></div>
   <div class="muted">Останавливается мягко (файл <code>transcribe.stop</code>), запускается без окна —
    как резидент в блоке «Резидент llm-host». Журнала у демона нет: смотрите
    <code>data/auto-transcribe*</code>.</div></section>

  <section><h2>Файлы out_dir</h2>
   <div class="row"><button onclick="loadTrList()">Обновить</button>
    <span id="trdir" class="muted"></span></div>
   <table class="tr"><thead><tr><th>Файл</th><th>Размер</th><th>Изменён</th><th>Спикеры</th><th>Имена</th></tr></thead>
    <tbody id="trrows"></tbody></table>
   <div class="muted" id="trhint">Выберите файл в списке — ниже откроется превью.</div></section>

  <section><h2>Превью</h2>
   <div class="row"><button id="trnames" onclick="openNames()" disabled>Присвоить имена спикерам</button>
    <button id="trapply" onclick="applyTask()" disabled>Перезапустить задание</button>
    <span id="trtitle" class="muted"></span></div>
   <pre id="trprev" class="muted">…</pre>
   <div class="muted" id="trappmsg"></div></section>

  <section><h2>Папки конвейера</h2>
   <div class="muted">Входная папка (исходные медиа) и выходная (транскрипты) —
    сохраняются в <code>config.yaml</code> и применяются к новым запускам демона.</div>
   <div class="row" style="margin-top:8px"><span class="muted" style="min-width:90px">inbox_dir</span>
    <input id="trinbox" placeholder="D:\\media_in" style="min-width:420px"></div>
   <div class="row" style="margin-top:6px"><span class="muted" style="min-width:90px">out_dir</span>
    <input id="trout" placeholder="D:\\media_out" style="min-width:420px"></div>
   <div class="row" style="margin-top:8px"><button onclick="saveTrDirs()">Сохранить папки</button>
    <span id="trdirmsg" class="muted"></span></div></section>

  <div class="modal" id="trmodal">
   <div class="box">
    <h3>Присвоить имена спикерам</h3>
    <div class="muted">Заполните только нужные строки — пустые останутся как есть:
     подставляются лишь введённые имена. Колонка «Спикер» берётся из файла.</div>
    <table class="tr"><thead><tr><th style="width:180px">Спикер</th><th>Имя</th></tr></thead>
     <tbody id="trspk"></tbody></table>
    <div class="row" style="margin-top:12px">
     <button onclick="applyNames()">Применить</button>
     <button onclick="closeNames()">Закрыть</button>
     <span id="trmsg" class="muted"></span></div>
   </div>
  </div>
 </div>

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
    if(document.getElementById('tab-search').style.display!=='none') loadWatchDaemon();
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
  const all=n.children||[], kids=all.slice(0,60);
  const more=all.length>60? '<div class="muted" style="margin-left:20px">… ещё '+(all.length-60)+' папок (показаны первые 60)</div>':'';
  return '<details'+(depth===0?' open':'')+'><summary>'+stIcon(n.status)+' <b>'+esc(n.name)+
    '</b> <span class="muted">(файлов '+n.files+', инд '+n.indexed+')</span></summary>'
    +kids.map(c=>nodeHtml(c,depth+1)).join('')+more+'</details>';
}
function treeAll(open){ Array.prototype.forEach.call(document.querySelectorAll('#tree details'),function(d){ d.open=open; }); }
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
async function restartLlmHost(){
  const m=document.getElementById('lhmsg');
  m.textContent='перезапускаю… (может занять до минуты)';
  try{ const r=await jpost('/api/llm-host/restart'); m.textContent=(r.ok?'':'ошибка: ')+(r.msg||(r.ok?'ok':'')); }
  catch(e){ m.textContent='ошибка: '+e; }
  refresh();
}
function clineIcon(s){ return s==='ok'?'&#9989;':(s==='skip'?'&#11036;':'&#9888;&#65039;'); }
async function clineSync(){
  const m=document.getElementById('clmsg'), b=document.getElementById('clres');
  m.textContent='синхронизирую…'; b.innerHTML='';
  try{ const r=await jpost('/api/cline/sync');
    m.textContent=r.error?(r.error):(r.ok?'готово':'есть предупреждения');
    b.innerHTML=(r.steps||[]).map(s=>'<div>'+clineIcon(s.status)+' <b>'+esc(s.title)+'</b>: '+esc(s.msg)+'</div>').join('')
      +(r.restart_required?'<div class="warnbox">&#9888;&#65039; '+esc(r.restart_note||'Перезапустите Cline.')+'</div>':'');
  }catch(e){ m.textContent='ошибка: '+e; }
}
// --- Транскрибация (PLAN_AUTO_TRANSCRIBE §8) ---
function showTab(name){
  const tr=(name==='transcribe');
  document.getElementById('tab-search').style.display=tr?'none':'block';
  document.getElementById('tab-transcribe').style.display=tr?'block':'none';
  Array.prototype.forEach.call(document.querySelectorAll('nav.tabs button'), function(b){
    b.classList.toggle('active', b.getAttribute('data-tab')===name); });
  try{ localStorage.setItem('hds.tab', name); }catch(e){}
  if(tr){ loadTrList(); loadTrDirs(); loadDaemon(); }
}
function fmtSize(n){ n=n||0; return n>1048576? (n/1048576).toFixed(1)+' МБ' : (n>1024? Math.round(n/1024)+' КБ' : n+' Б'); }
function fmtTime(t){ return t? new Date(t*1000).toLocaleString() : '—'; }
let trFile=null, trData=null;
async function loadTrList(){
  const tb=document.getElementById('trrows'), d=document.getElementById('trdir');
  tb.innerHTML='<tr><td colspan=5 class="muted">…</td></tr>';
  try{ const j=await jget('/api/transcribe/list');
    d.textContent=(j.dir||'папка не задана')+(j.error? ' · '+j.error : '');
    const f=j.files||[];
    if(!f.length){ tb.innerHTML='<tr><td colspan=5 class="muted">нет файлов</td></tr>'; return; }
    tb.innerHTML=f.map(function(x){
      return '<tr data-name="'+esc(x.name)+'"><td>'+esc(x.name)+'</td><td class="muted">'+fmtSize(x.size)+
        '</td><td class="muted">'+esc(fmtTime(x.mtime))+'</td><td class="muted">'+esc((x.speakers||[]).join(', '))+
        '</td><td>'+(x.renamed? '<span class="ok">имена присвоены</span>' : '<span class="muted">не присвоены</span>')+'</td></tr>';
    }).join('');
    Array.prototype.forEach.call(tb.querySelectorAll('tr'), function(r){
      r.onclick=function(){ openTrFile(r.getAttribute('data-name')); }; });
  }catch(e){ tb.innerHTML='<tr><td colspan=5 class="err">ошибка: '+esc(''+e)+'</td></tr>'; }
}
async function openTrFile(name){
  trFile=name;
  const p=document.getElementById('trprev'), t=document.getElementById('trtitle');
  p.textContent='…'; t.textContent=name; t.title='';
  try{ const j=await jget('/api/transcribe/file?name='+encodeURIComponent(name));
    if(j.error){ p.textContent='ошибка: '+j.error; trData=null;
      document.getElementById('trnames').disabled=true; document.getElementById('trapply').disabled=true; return; }
    trData=j; p.textContent=j.text||''; t.textContent=j.name; t.title=j.path||'';
    document.getElementById('trnames').disabled=false;
    document.getElementById('trapply').disabled=false;
    document.getElementById('trappmsg').textContent='';
    Array.prototype.forEach.call(document.querySelectorAll('#trrows tr'), function(r){
      r.classList.toggle('sel', r.getAttribute('data-name')===name); });
  }catch(e){ p.textContent='ошибка: '+e; }
}
function openNames(){
  if(!trFile||!trData){ alert('Сначала выберите файл в списке'); return; }
  const saved=trData.saved||{}, spk=trData.speakers||[];
  const rows=spk.map(function(s){
    return '<tr><td><code>'+esc(s)+'</code></td><td><input data-sp="'+esc(s)+'" value="'+esc(saved[s]||'')+
      '" placeholder="Имя или метка"></td></tr>';
  }).join('');
  document.getElementById('trspk').innerHTML=rows||'<tr><td colspan=2 class="muted">метки не найдены</td></tr>';
  document.getElementById('trmsg').textContent='';
  document.getElementById('trmodal').classList.add('open');
}
function closeNames(){ document.getElementById('trmodal').classList.remove('open'); }
async function applyNames(){
  const names={};
  Array.prototype.forEach.call(document.querySelectorAll('#trspk input[data-sp]'), function(i){
    names[i.getAttribute('data-sp')]=i.value; });
  const m=document.getElementById('trmsg'); m.textContent='…';
  try{ const r=await jpost('/api/transcribe/speakers', {name: trFile, names: names});
    m.textContent=r.msg||r.error||'ok';
    if(r.ok){ await openTrFile(trFile); await loadTrList(); openNames(); }
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function applyTask(){
  if(!trFile){ return; }
  const m=document.getElementById('trappmsg'); m.textContent='…';
  try{ const r=await jpost('/api/transcribe/apply', {name: trFile});
    m.textContent=(r.ok? '' : 'ошибка: ')+(r.msg||r.error||'ok');
    if(r.ok) setTimeout(loadTrList, 1500);
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function loadTrDirs(){
  try{ const c=await jget('/api/config'); const a=c.auto_transcribe||{};
    document.getElementById('trinbox').value=a.inbox_dir||'';
    document.getElementById('trout').value=a.out_dir||'';
  }catch(e){}
}
async function saveTrDirs(){
  const m=document.getElementById('trdirmsg'); m.textContent='…';
  const body={inbox_dir: document.getElementById('trinbox').value, out_dir: document.getElementById('trout').value};
  try{ const r=await jpost('/api/config/transcribe-dirs', body);
    m.textContent=(r.ok? '' : 'ошибка: ')+(r.msg||r.error||'ok')+(r.warn? ' · внимание: '+r.warn : '');
    if(r.ok) loadTrList();
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function loadDaemon(){
  const el=document.getElementById('trdaemon');
  try{ const d=await jget('/api/transcribe/daemon');
    const st=d.running? '<span class="ok">работает</span>'+(d.pid? ' (pid '+d.pid+')' : '')
      : (d.stale? '<span class="err">lock без процесса</span>' : '<span class="muted">остановлен</span>');
    el.innerHTML='Состояние: '+st+' · в конфиге: '+(d.enabled? 'включён' : '<span class="err">выключен</span>')
      +' · папки: <span class="muted">'+esc(d.inbox_dir||'—')+' → '+esc(d.out_dir||'—')+'</span>'
      +' · опрос каждые '+(d.poll_seconds||'?')+' с';
  }catch(e){ el.textContent='ошибка: '+e; }
}
async function daemonAct(action){
  const m=document.getElementById('trdaemonmsg'); m.textContent='…';
  try{ const r=await jpost('/api/transcribe/daemon', {action: action});
    m.textContent=(r.ok? '' : 'ошибка: ')+(r.msg||r.error||'ok')+(r.warn? ' · '+r.warn : '');
    await loadDaemon();
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function loadWatchDaemon(){
  const el=document.getElementById('wdaemon');
  try{ const d=await jget('/api/watch/daemon');
    const st=d.running? '<span class="ok">работает</span>'+(d.pid? ' (pid '+d.pid+')' : '')
      : (d.stale? '<span class="err">lock без процесса</span>' : '<span class="muted">остановлен</span>');
    let extra='';
    if(d.paused) extra+=' · <span class="warnbox">пауза индексации</span>';
    if(d.stop_requested) extra+=' · <span class="err">запрошена остановка (index.stop)</span>';
    if(d.indexing) extra+=' · индексация идёт';
    el.innerHTML='Состояние: '+st+extra+' · корней в индексе: '+((d.roots||[]).length);
  }catch(e){ el.textContent='ошибка: '+e; }
}
async function watchAct(action){
  const m=document.getElementById('wdaemonmsg'); m.textContent='…';
  try{ const r=await jpost('/api/watch/daemon', {action: action});
    m.textContent=(r.ok? '' : 'ошибка: ')+(r.msg||r.error||'ok');
    await loadWatchDaemon();
  }catch(e){ m.textContent='ошибка: '+e; }
}
function initTab(){
  let t='search';
  try{ t=localStorage.getItem('hds.tab')||'search'; }catch(e){}
  showTab(t==='transcribe'?'transcribe':'search');
  loadWatchDaemon();
}
initTab();
refresh(); setInterval(refresh, 3000); loadDiag(); loadTree(); loadCfg();
</script></body></html>
"#;
