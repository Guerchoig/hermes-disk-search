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
 input,button{font:inherit;padding:8px 10px;border-radius:8px;border:1px solid #2a3a55;background:#0d1620;color:#e6edf3}
 input{min-width:280px}button{cursor:pointer;background:#1c2f44}
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

 <section><h2>Поиск</h2>
  <div class="row"><input id="q" placeholder="например: накладная склад">
   <input id="lim" type="number" value="8" min="1" max="30" style="min-width:70px">
   <button onclick="doSearch()">Найти</button></div>
  <div id="sres"></div></section>

 <section><h2>Вопрос (ответ по файлам)</h2>
  <div class="row"><input id="aq" placeholder="о чём … ?" style="min-width:360px">
   <button onclick="doAsk()">Спросить</button></div>
  <pre id="aans" class="muted" style="display:none"></pre></section>

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
async function jpost(u){ const r=await fetch(u,{method:'POST'}); return await r.json(); }
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
refresh(); setInterval(refresh, 3000);
</script></body></html>
"#;
