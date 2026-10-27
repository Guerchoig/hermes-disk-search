//! Встроенная страница Rust-UI (одна HTML, без внешних ресурсов). Перепроектирована
//! под Rust-стек: статус, администрирование (индексация, резидент `llm-host`, модель
//! чата, MCP-сервер, транскрибация, настройки). Python-`assets/ui.html` остаётся для
//! Python-версии.

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
 input,button,textarea,select{font:inherit;padding:8px 10px;border-radius:8px;border:1px solid #2a3a55;background:#0d1620;color:#e6edf3}
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
  <button data-tab="search" class="active" onclick="showTab('search')">Главная</button>
  <button data-tab="transcribe" onclick="showTab('transcribe')">Транскрибация</button>
  <button data-tab="anonymize" onclick="showTab('anonymize')">Анонимизация</button>
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

 <section><h2>Модель чата (общий llama-рантайм)</h2>
  <div class="row"><select id="cm" style="min-width:280px"></select>
   <button onclick="applyChatModel()">Сменить и перезапустить llm-host</button>
   <span id="cmmsg" class="muted"></span></div>
  <div id="cminfo" class="muted">…</div></section>

 <section><h2>MCP-сервер disk-search (агенты)</h2>
  <div class="row">
   <button onclick="mcpAct('start')">Запустить</button>
   <button onclick="mcpAct('stop')">Остановить</button>
   <button onclick="mcpAct('restart')">Перезапустить</button>
   <span id="mcpmsg" class="muted"></span></div>
  <div id="mcpstatus" class="muted">…</div>
  <div class="muted">Общий инстанс streamable-http (<code>mcp_http.*</code> конфига,
   по умолчанию <code>127.0.0.1:8787/mcp</code>): к нему подключаются Cline/Hermes.
   «Чужой» сервис на порту не переиспользуется — освободите порт и запустите заново.</div></section>

 <section><h2>Службы (server-профиль)</h2>
  <div id="svc-list" class="muted">…</div>
  <div class="row" style="margin-top:8px"><button onclick="loadServices()">Обновить состояние</button>
   <span id="svcmsg" class="muted"></span></div>
  <div class="muted">Установка/запуск OS-служб (<code>hermes-disk-search-&lt;роль&gt;</code>):
   доступно только в server-профиле и требует прав администратора (или службы LocalSystem).</div></section>

 <section><h2>Доступ (пользователи и проекты)</h2>
  <div id="access-users" class="muted">…</div>
  <div class="row" style="margin-top:8px">
   <input id="acc-login" placeholder="логин" style="min-width:110px">
   <input id="acc-pass" placeholder="пароль" type="password" style="min-width:110px">
   <input id="acc-projects" placeholder="проекты: A,B" style="min-width:190px">
   <label style="display:flex;gap:4px;align-items:center"><input id="acc-admin" type="checkbox" style="min-width:auto"> админ</label>
   <button onclick="accessAdd()">Создать</button>
   <button onclick="accessDel()">Удалить</button>
   <button onclick="accessList()">Обновить</button>
  </div>
  <div id="accmsg" class="muted"></div>
  <div class="row" style="margin-top:8px">
   <input id="tok-user" placeholder="логин" style="min-width:110px">
   <input id="tok-ttl" placeholder="TTL дней" style="min-width:90px">
   <button onclick="accessTokenNew()">Выдать токен</button>
   <span id="tokmsg" class="muted"></span>
  </div>
  <pre id="tokout" class="muted" style="display:none"></pre>
  <div class="muted">Проекты назначаются по корням (<code>index.roots</code>: «корень → проект»).
   В server-режиме страница открывается только с админ-токеном (<code>?token=...</code> —
   запоминается в браузере; токены выдаёт <code>POST /auth/login</code> или <code>hds login</code>).</div>
 </section>

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
    <input id="trinbox" placeholder="D:\\media_in" style="min-width:420px" oninput="trInd('trinbox','trinboxind')">
    <span id="trinboxind" class="muted"></span></div>
   <div class="row" style="margin-top:6px"><span class="muted" style="min-width:90px">out_dir</span>
    <input id="trout" placeholder="D:\\media_out" style="min-width:420px" oninput="trInd('trout','troutind')">
    <span id="troutind" class="muted"></span></div>
   <div class="muted" style="margin-top:6px">Индикатор: папка исключена из индексации (неявно через
    <code>auto_transcribe.*</code> или явно в <code>index.exclude_paths</code>)? Медиа индексировать
    не нужно — достаточно исключить папки конвейера.</div>
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

 <div id="tab-anonymize" hidden>
  <section><h2>Анонимизация документов</h2>
   <div class="muted">Вход — файл или папка (папка обрабатывается по верхнему уровню).
    Выходной каталог — <code>anonymize.output_dir</code> из конфига: его можно изменить
    и сохранить. Форматы: docx, xlsx, txt, md, xml.</div>
   <div class="row" style="margin-top:8px"><span class="muted" style="min-width:70px">путь</span>
    <input id="anopath" placeholder="D:\in" style="min-width:420px"
     onkeydown="if(event.key==='Enter')loadAnonLs()">
    <button onclick="loadAnonLs()">Показать файлы</button>
    <button onclick="openAnonBrowse()">Обзор…</button>
    <span id="anolsmsg" class="muted"></span></div>
   <div class="row" style="margin-top:6px"><span class="muted" style="min-width:70px">out_dir</span>
    <input id="anoutdir" style="min-width:420px">
    <button onclick="saveAnonOutDir()">Сохранить</button>
    <span id="anoutmsg" class="muted"></span></div>
   <div class="muted" style="margin-top:4px">каталог выхода: <code id="anoutcur">…</code></div>
   <div id="anoninfo" class="muted">…</div></section>

  <section><h2>Найденные файлы</h2>
   <div class="row"><button onclick="loadAnonLs()">Обновить список</button>
    <span id="anoncount" class="muted"></span></div>
   <table class="tr"><thead><tr><th>Файл</th><th>Размер</th><th>Изменён</th><th>Станет</th></tr></thead>
    <tbody id="anonrows"><tr><td colspan=4 class="muted">введите путь или выберите папку кнопкой «Обзор…»</td></tr></tbody></table>
   <div class="muted">Клик по строке подставляет этот файл в поле входа — обработается только он.
    Файлы с пометкой «уже обработан» — результат прежнего акта (повтор даст warning).</div></section>

  <section><h2>Действия</h2>
   <div class="muted">Список ниже используется только кнопками «Скрыть указанное» и
    «Показать указанное»: для скрытия — значения PII, для показа — токены вида
    <code>[PERSON_1]</code>, по одному на строку. Кнопки «Скрыть всё» и «Показать всё»
    этот список не используют. В отчёт попадают только токены и счётчики — значения PII
    не выводятся.</div>
   <div class="muted" style="margin-top:4px">Одновременно выполняется только одно действие:
    пока оно не завершено, все четыре кнопки заблокированы. Следующее можно запустить после
    завершения.</div>
   <textarea id="anonitems" spellcheck="false"
    placeholder="для «скрыть указанное» — значения, для «показать указанное» — токены [PERSON_1]; по одному на строку"></textarea>
   <div class="row" style="margin-top:8px">
    <button id="anbtn1" onclick="anonRun('anonymize')"
     title="найти PII во всех файлах и заменить токенами (список не нужен)">Скрыть всё</button>
    <button id="anbtn2" onclick="anonRun('values')"
     title="заменить только значения из списка">Скрыть указанное</button>
    <button id="anbtn3" onclick="anonRun('deanonymize')"
     title="вернуть значения по всем токенам базы (список не нужен)">Показать всё</button>
    <button id="anbtn4" onclick="anonRun('deanon-values')"
     title="вернуть только токены из списка">Показать указанное</button>
    <span id="anrunmsg" class="muted"></span></div>
   <div id="anprogress" class="muted">готов к запуску</div></section>

  <section><h2>Отчёт</h2>
   <div id="anreport" class="muted">отчёта пока нет</div></section>

  <div class="modal" id="anbmodal">
   <div class="box">
    <h3>Выбор файла или папки на сервере</h3>
    <div class="muted">Показаны папки и файлы поддерживаемых форматов. Клик по папке
     открывает её, клик по файлу подставляет его в поле входа. Кнопка «Выбрать эту папку»
     подставляет текущий каталог.</div>
    <div class="row" style="margin-top:8px">
     <button id="anbup" onclick="anbUp()" disabled>Вверх</button>
     <input id="anbpath" style="min-width:320px" onkeydown="if(event.key==='Enter')anbGo()">
     <button onclick="anbGo()">Перейти</button></div>
    <div id="anbmsg" class="muted">…</div>
    <table class="tr"><thead><tr><th>Имя</th><th>Тип</th></tr></thead>
     <tbody id="anbrows"></tbody></table>
    <div class="row" style="margin-top:12px">
     <button id="anbpick" onclick="anbPickDir()" disabled>Выбрать эту папку</button>
     <button onclick="closeAnonBrowse()">Отмена</button></div>
   </div>
  </div>
 </div>

</main>
<script>
// M1 T5: админ-токен UI (?token=... сохраняется; сервер-режим требует его).
const HDS_T=(function(){ try{
  const p=new URLSearchParams(location.search).get('token');
  if(p){ localStorage.setItem('hds.token',p); history.replaceState(null,'',location.pathname); }
}catch(e){} try{ return localStorage.getItem('hds.token')||''; }catch(e){ return ''; } })();
function hdsAuth(){ return HDS_T? {'Authorization':'Bearer '+HDS_T} : {}; }
async function jget(u){ const r=await fetch(u,{headers:hdsAuth()});
  if(r.status===401) throw new Error('401: нужен админ-токен (откройте страницу с ?token=...)');
  return await r.json(); }
async function jpost(u,body){ const r=await fetch(u,{method:'POST',
  headers:Object.assign({'Content-Type':'application/json','X-HDS-UI':'1'}, hdsAuth()),
  body:JSON.stringify(body||{})});
  if(r.status===401) throw new Error('401: нужен админ-токен (откройте страницу с ?token=...)');
  return await r.json(); }
function esc(s){ return (s||'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])); }
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
    if(document.getElementById('tab-search').style.display!=='none') { loadWatchDaemon(); accessList(); }
  }catch(e){ document.getElementById('status').textContent='ошибка статуса: '+e; }
}
async function loadChatModels(){
  const sel=document.getElementById('cm'), inf=document.getElementById('cminfo');
  try{ const j=await jget('/api/chat-model');
    if(j.error){ inf.innerHTML='<span class="err">'+esc(j.error)+'</span>'; return; }
    sel.innerHTML=(j.models||[]).map(m=>'<option'+(m.current?' selected':'')+' value="'+esc(m.file)+'">'+
      esc(m.file)+(m.size_mb?' ('+m.size_mb+' МиБ':'')+(m.parts>1?', '+m.parts+' частей':'')+(m.size_mb?')':'')+'</option>').join('')||'<option value="">(нет .gguf в каталоге)</option>';
    inf.innerHTML='каталог: '+esc(j.dir||'?')+' · активная: <b>'+esc(j.current||'(манифест не создан)')+'</b>'+
      ' · llm-host: '+(j.host_up?'<span class="ok">up</span>':'<span class="err">не отвечает</span>')+
      (j.warn?' · <span class="warnbox">'+esc(j.warn)+'</span>':'')+
      '<br>Смена перезаписывает <code>current.json</code> и перезапускает резидент (аккуратно: '+
      'устаревшие pid-файлы и зависшие процессы разбираются). Первый ответ после смены — с задержкой.';
    try{ const j=await jget('/api/llm-host/job');
      if(j.running){ const m=document.getElementById('cmmsg');
        m.textContent='… '+j.stage+' · '+j.detail+' ('+j.elapsed_sec+' с)'; pollJob(m); }
    }catch(e){}
  }catch(e){ inf.textContent='ошибка: '+e; }
}
let jobTimer=null;
async function pollJob(m){
  try{ const j=await jget('/api/llm-host/job');
    if(j.running){
      m.textContent='… '+j.stage+' · '+j.detail+' ('+j.elapsed_sec+' с)';
      if(!jobTimer) jobTimer=setInterval(()=>pollJob(m), 2000);
      return;
    }
    const r=j.result||{};
    m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'готово');
  }catch(e){ m.textContent='ошибка: '+e; }
  if(jobTimer){ clearInterval(jobTimer); jobTimer=null; }
  loadChatModels(); loadMcp(); refresh();
}
async function applyChatModel(){
  const f=document.getElementById('cm').value; if(!f) return;
  const m=document.getElementById('cmmsg'); m.textContent='записываю модель…';
  try{ const r=await jpost('/api/chat-model/set',{file:f});
    if(r.started){ m.textContent='… перезапуск llm-host запущен'; pollJob(m); }
    else m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'ok');
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function loadMcp(){
  const el=document.getElementById('mcpstatus');
  try{ const j=await jget('/api/mcp-http');
    const st=j.state==='mcp'?'<span class="ok">работает</span>'
      :(j.state==='foreign'?'<span class="err">порт занят чужим сервисом</span>':'<span class="muted">остановлен</span>');
    el.innerHTML='Состояние: '+st+(j.pid?' · pid '+j.pid:'')+' · URL: '+esc(j.url||'?')+
      (j.version?' · версия '+esc(j.version):'');
  }catch(e){ el.textContent='ошибка: '+e; }
}
async function mcpAct(action){
  const m=document.getElementById('mcpmsg'); m.textContent='…';
  try{ const r=await jpost('/api/mcp-http',{action:action});
    m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'ok');
  }catch(e){ m.textContent='ошибка: '+e; }
  loadMcp(); refresh();
}
async function loadServices(){
  const el=document.getElementById('svc-list');
  try{ const j=await jget('/api/services');
    if(j.profile!=='server'){ el.innerHTML='<span class="muted">'+esc(j.hint||'службы только в server-профиле')+'</span>'; return; }
    const st=function(s){ return s==='running'?'<span class="ok">работает</span>'
      :(s==='stopped'?'<span class="muted">остановлена</span>'
      :(s==='absent'?'<span class="muted">не установлена</span>':'<span class="err">неизвестно</span>')); };
    const rows=(j.services||[]).map(function(s){
      const acts=['install','uninstall','start','stop','restart'].map(function(a){
        return '<button onclick="svcAct(\''+esc(s.role)+'\',\''+a+'\')">'+a+'</button>'; }).join(' ');
      return '<tr><td><code>'+esc(s.role)+'</code></td><td>'+st(s.state)+'</td><td class="muted">'+
        (s.autostart?'да':'нет')+'</td><td>'+acts+'</td></tr>'; }).join('');
    el.innerHTML='<table class="tr"><thead><tr><th>Роль</th><th>Состояние</th><th>Автозапуск</th><th>Действия</th></tr></thead><tbody>'
      +(rows||'<tr><td colspan=4 class="muted">нет ролей</td></tr>')+'</tbody></table>';
  }catch(e){ el.textContent='ошибка: '+e; }
}
async function svcAct(role, action){
  const m=document.getElementById('svcmsg'); m.textContent='… '+action+' '+role;
  try{ const r=await jpost('/api/services', {role:role, action:action});
    m.textContent=(r.ok? '' : 'ошибка: ')+(r.msg||r.hint||r.error||'ok');
  }catch(e){ m.textContent='ошибка: '+e; }
  loadServices();
}
async function accessList(){
  const el=document.getElementById('access-users');
  try{ const j=await jget('/api/access/users');
    if(j.error){ el.innerHTML='<span class="err">'+esc(j.error)+'</span>'; return; }
    const us=j.users||[];
    if(!us.length){ el.innerHTML='<span class="muted">пользователей нет</span>'; return; }
    el.innerHTML='<table class="tr"><thead><tr><th>Логин</th><th>Имя</th><th>Админ</th><th>Проекты</th></tr></thead><tbody>'
      +us.map(function(u){ return '<tr><td><code>'+esc(u.login)+'</code></td><td>'+esc(u.name||'')+'</td><td>'+
        (u.admin?'да':'')+(u.disabled?' <span class="err">отключён</span>':'')+'</td><td class="muted">'+
        esc((u.projects||[]).join(', ')||'—')+'</td></tr>'; }).join('')+'</tbody></table>';
  }catch(e){ el.innerHTML='<span class="err">ошибка: '+esc(''+e)+'</span>'; }
}
async function accessAdd(){
  const m=document.getElementById('accmsg');
  const login=(document.getElementById('acc-login').value||'').trim();
  const password=document.getElementById('acc-pass').value||'';
  const projects=(document.getElementById('acc-projects').value||'').split(',').map(function(s){return s.trim();}).filter(Boolean);
  const admin=document.getElementById('acc-admin').checked;
  if(!login||!password){ m.textContent='нужны логин и пароль'; return; }
  m.textContent='…';
  try{ const r=await jpost('/api/access/user/add', {login:login, password:password, admin:admin, projects:projects});
    m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'ok');
  }catch(e){ m.textContent='ошибка: '+e; }
  accessList();
}
async function accessDel(){
  const m=document.getElementById('accmsg');
  const login=(document.getElementById('acc-login').value||'').trim();
  if(!login){ m.textContent='укажите логин в поле «логин»'; return; }
  m.textContent='…';
  try{ const r=await jpost('/api/access/user/del', {login:login});
    m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'ok');
  }catch(e){ m.textContent='ошибка: '+e; }
  accessList();
}
async function accessTokenNew(){
  const m=document.getElementById('tokmsg'), o=document.getElementById('tokout');
  const user=(document.getElementById('tok-user').value||'').trim();
  if(!user){ m.textContent='укажите логин'; return; }
  const ttl=parseInt(document.getElementById('tok-ttl').value,10);
  const body={user:user}; if(ttl>0) body.ttl_days=ttl;
  m.textContent='…'; o.style.display='none'; o.textContent='';
  try{ const r=await jpost('/api/access/token/new', body);
    if(r.token){ m.textContent='токен выдан — показывается один раз';
      o.textContent='Токен для '+user+':\n'+r.token+'\nИстекает: '+fmtTime(r.expires); o.style.display='block'; }
    else m.textContent='ошибка: '+(r.error||r.msg||'не удалось');
  }catch(e){ m.textContent='ошибка: '+e; }
  accessList();
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
  try{ const r=await jpost('/api/llm-host/restart');
    if(r.started){ m.textContent='… перезапуск запущен'; pollJob(m); }
    else m.textContent=(r.ok?'':'ошибка: ')+(r.msg||'ok');
  }
  catch(e){ m.textContent='ошибка: '+e; }
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
  const tabs=['search','transcribe','anonymize'];
  const cur=tabs.indexOf(name)>=0? name : 'search';
  tabs.forEach(function(t){
    document.getElementById('tab-'+t).style.display=(t===cur?'block':'none'); });
  Array.prototype.forEach.call(document.querySelectorAll('nav.tabs button'), function(b){
    b.classList.toggle('active', b.getAttribute('data-tab')===cur); });
  try{ localStorage.setItem('hds.tab', cur); }catch(e){}
  if(cur==='transcribe'){ loadTrList(); loadTrDirs(); loadDaemon(); }
  if(cur==='anonymize'){
    if(document.getElementById('anopath').value.trim()) loadAnonLs(); else loadAnonInfo();
    loadAnonStatus();
  }
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
    // R2.5: неявные (auto_transcribe.*) + явные (index.exclude_paths) исключения.
    trExcl=(c.implicit_excludes||[]).concat(c.exclude_paths||[]);
    document.getElementById('trinbox').value=a.inbox_dir||'';
    document.getElementById('trout').value=a.out_dir||'';
    trInd('trinbox','trinboxind'); trInd('trout','troutind');
  }catch(e){}
}
// R2.5: индикатор «папка исключена из индексации ✓/✗» у полей папок.
let trExcl=[];
function trNorm(p){ return String(p||'').trim().replace(/\\/g,'/').replace(/\/+$/,'').toLowerCase(); }
function trInd(inputId, spanId){
  const el=document.getElementById(spanId); if(!el){ return; }
  const v=trNorm(document.getElementById(inputId).value);
  if(!v){ el.innerHTML=''; return; }
  const hit=trExcl.some(function(x){ return trNorm(x)===v; });
  el.innerHTML=hit? '<span class="ok">&#9989; исключена из индексации</span>'
    : '<span class="err">&#10060; НЕ исключена из индексации</span>';
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
    // R3.9: форма управления — служба SCM / задача Планировщика / процесс UI.
    const form=d.form==='service'? '<b>служба SCM</b> (её же видно в карточке «Службы»)'
      : (d.form==='task'? '<b>задача Планировщика</b> HermesDiskSearchTranscribe'
         : '<b>процесс</b> (поднимает эта кнопка)');
    let svc='';
    if(d.service&&d.service.state){ svc=' · SCM: '+esc(d.service.state)+(d.service.autostart? ', автостарт':''); }
    el.innerHTML='Форма: '+form+' · состояние: '+st+svc+' · в конфиге: '+(d.enabled? 'включён' : '<span class="err">выключен</span>')
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
// --- Анонимизация (PLAN_ANONYMIZER_SIDECAR §2.5) ---
let anTimer=null;
function anButtons(on){
  ['anbtn1','anbtn2','anbtn3','anbtn4'].forEach(function(id){
    const b=document.getElementById(id); if(b) b.disabled=!!on; });
}
function anonItems(){
  const t=document.getElementById('anonitems').value||'';
  return t.split('\n').map(function(s){ return s.trim(); }).filter(function(s){ return s.length>0; });
}
function anonInfoHtml(j){
  return 'форматы: '+esc((j.formats||[]).join(', '))+
    (j.formats_source==='fallback'? ' <span class="warnbox">воркер недоступен — форматы по умолчанию</span>':'')+
    (j.db_path? '<br>БД токенов: <code>'+esc(j.db_path)+'</code>':'');
}
async function loadAnonInfo(){
  // Данные карточки без обзора пути: каталог выхода и форматы (ошибку «путь не задан»
  // при открытии закладки не показываем).
  try{ const j=await jget('/api/anonymize/ls?path=');
    if(j.out_dir){ document.getElementById('anoutdir').value=j.out_dir; anonOutDirLabel(j.out_dir); }
    document.getElementById('anoninfo').innerHTML=anonInfoHtml(j);
  }catch(e){}
}
async function loadAnonLs(){
  const p=document.getElementById('anopath').value.trim();
  const m=document.getElementById('anolsmsg'); m.textContent='…';
  try{ const j=await jget('/api/anonymize/ls?path='+encodeURIComponent(p));
    if(j.out_dir){ document.getElementById('anoutdir').value=j.out_dir; anonOutDirLabel(j.out_dir); }
    document.getElementById('anoninfo').innerHTML=anonInfoHtml(j);
    if(j.error) m.innerHTML='<span class="err">'+esc(j.error)+'</span>';
    else m.textContent='файлов: '+((j.files||[]).length);
    const tb=document.getElementById('anonrows');
    const f=j.files||[];
    document.getElementById('anoncount').textContent='найдено: '+f.length;
    if(!f.length){ tb.innerHTML='<tr><td colspan=4 class="muted">'+esc(j.note||'нет файлов')+'</td></tr>'; return; }
    tb.innerHTML=f.map(function(x){
      return '<tr data-path="'+esc(x.path)+'" title="подставить этот файл во вход"><td>'+esc(x.name)+
        (x.processed?' <span class="warnbox">уже обработан</span>':'')+
        '</td><td class="muted">'+fmtSize(x.size)+'</td><td class="muted">'+esc(fmtTime(x.mtime))+
        '</td><td class="muted">'+esc(x.anonymized_out||'')+'</td></tr>';
    }).join('');
    Array.prototype.forEach.call(tb.querySelectorAll('tr'), function(r){
      r.onclick=function(){ document.getElementById('anopath').value=r.getAttribute('data-path'); loadAnonLs(); };
    });
  }catch(e){ m.innerHTML='<span class="err">ошибка: '+esc(''+e)+'</span>'; }
}
function anonOutDirLabel(d){
  const el=document.getElementById('anoutcur');
  if(el) el.textContent=d||'?';
}
async function saveAnonOutDir(){
  const inp=document.getElementById('anoutdir'), v=inp.value.trim();
  const m=document.getElementById('anoutmsg'); m.textContent='…';
  try{ const r=await jpost('/api/config/anonymize-dirs',{out_dir:v});
    m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'ok');
    if(r.ok){ const d=r.out_dir||v; inp.value=d; anonOutDirLabel(d);
      // «Станет» в списке файлов считается от out_dir — обновляем, если путь уже введён.
      if(document.getElementById('anopath').value.trim()) loadAnonLs(); }
  }catch(e){ m.textContent='ошибка: '+e; }
}
// Выбор файла/папки на сервере: браузер не отдаёт абсолютный путь из <input type=file>,
// поэтому каталоги листает сервер (`GET /api/anonymize/browse`).
let anbCur='', anbParent=null;
async function openAnonBrowse(){
  document.getElementById('anbmodal').classList.add('open');
  await loadAnonBrowse(document.getElementById('anopath').value.trim());
}
function closeAnonBrowse(){ document.getElementById('anbmodal').classList.remove('open'); }
async function anbGo(){ await loadAnonBrowse(document.getElementById('anbpath').value.trim()); }
async function anbUp(){ if(anbParent) await loadAnonBrowse(anbParent); }
function anonPickPath(p){
  document.getElementById('anopath').value=p;
  closeAnonBrowse();
  loadAnonLs();
}
function anbPickDir(){ if(anbCur) anonPickPath(anbCur); }
async function loadAnonBrowse(p){
  const m=document.getElementById('anbmsg'), tb=document.getElementById('anbrows');
  m.textContent='…'; tb.innerHTML='<tr><td colspan=2 class="muted">…</td></tr>';
  try{ const j=await jget('/api/anonymize/browse?path='+encodeURIComponent(p||''));
    anbCur=j.path||''; anbParent=j.parent||null;
    document.getElementById('anbpath').value=anbCur;
    document.getElementById('anbup').disabled=!anbParent;
    document.getElementById('anbpick').disabled=!anbCur;
    if(j.error) m.innerHTML='<span class="err">'+esc(j.error)+'</span>';
    else if(!anbCur) m.textContent='выберите диск или введите путь';
    else m.textContent='папок: '+((j.dirs||[]).length)+' · файлов: '+((j.files||[]).length)+
      (j.truncated?' (список обрезан)':'');
    const rows=[];
    (j.roots||[]).forEach(function(r){
      rows.push('<tr data-dir="'+esc(r.path)+'"><td><b>'+esc(r.name)+'</b></td>'+
        '<td class="muted">диск</td></tr>');
    });
    (j.dirs||[]).forEach(function(d){
      rows.push('<tr data-dir="'+esc(d.path)+'"><td>&#128193; '+esc(d.name)+'</td>'+
        '<td class="muted">папка</td></tr>');
    });
    (j.files||[]).forEach(function(f){
      rows.push('<tr data-file="'+esc(f.path)+'"><td>'+esc(f.name)+'</td>'+
        '<td class="muted">'+esc(f.ext)+' · '+fmtSize(f.size)+'</td></tr>');
    });
    tb.innerHTML=rows.join('')||'<tr><td colspan=2 class="muted">пусто</td></tr>';
    Array.prototype.forEach.call(tb.querySelectorAll('tr'), function(r){
      const d=r.getAttribute('data-dir'), f=r.getAttribute('data-file');
      if(!d && !f) return;
      r.onclick=function(){ if(f){ anonPickPath(f); } else { loadAnonBrowse(d); } };
    });
  }catch(e){ m.innerHTML='<span class="err">ошибка: '+esc(''+e)+'</span>'; }
}
async function anonRun(action){
  const p=document.getElementById('anopath').value.trim();
  const m=document.getElementById('anrunmsg');
  if(!p){ m.textContent='укажите путь (файл или папка)'; return; }
  const items=anonItems();
  if((action==='values'||action==='deanon-values') && !items.length){
    m.textContent='для «указанного» заполните список (по одному на строку)'; return; }
  m.textContent='запуск…'; anButtons(true);
  try{ const r=await jpost('/api/anonymize/run',{path:p,action:action,items:items});
    m.textContent=(r.ok?'':'ошибка: ')+(r.msg||r.error||'ok');
    if(r.started) pollAnon(); else anButtons(false);
  }catch(e){ m.textContent='ошибка: '+e; anButtons(false); }
}
async function loadAnonStatus(){
  try{ const j=await jget('/api/anonymize/status'); anonShow(j); }catch(e){}
}
async function pollAnon(){
  try{ const j=await jget('/api/anonymize/status'); anonShow(j);
    if(j.running){ if(!anTimer) anTimer=setInterval(pollAnon, 2000); }
    else if(anTimer){ clearInterval(anTimer); anTimer=null; loadAnonLs(); }
  }catch(e){ document.getElementById('anprogress').textContent='ошибка статуса: '+e; }
}
function anonTokens(t){
  return t.map(function(x){ return esc(x.token)+'×'+x.count; }).join(', ');
}
function anonShow(j){
  const p=document.getElementById('anprogress'), r=document.getElementById('anreport');
  if(j.running){
    anButtons(true);
    p.textContent='… '+esc(j.title||j.action||'')+' · '+esc(j.stage||'')+
      (j.detail? ' · '+j.detail : '')+' · файлов: '+j.done+'/'+j.total+
      (j.current? ' · текущий: '+j.current : '')+' · '+j.elapsed_sec+' с';
  } else { anButtons(false);
    p.textContent=j.total? 'акт завершён ('+j.elapsed_sec+' с)' : 'готов к запуску';
  }
  const res=j.result||null;
  if(!res){ if(!j.running) r.innerHTML='<span class="muted">отчёта пока нет</span>'; return; }
  const files=res.files||[];
  let h='<div class="row"><b>'+esc(res.title||'')+'</b>: '+esc(res.msg||'')+
    ' · файлов: '+(res.total||files.length)+' · неудач: '+(res.failed||0)+
    ' · устройство: '+esc(res.device||'')+'</div>';
  if(res.lease) h+='<div class="warnbox">'+esc(res.lease)+'</div>';
  h+='<table class="tr"><thead><tr><th>Файл</th><th>Выход</th><th>Токены</th><th>Итог</th></tr></thead><tbody>';
  h+=files.map(function(f){
    let toks='—';
    if(f.tokens&&f.tokens.length) toks=anonTokens(f.tokens);
    else if(f.restored&&f.restored.length) toks='восстановлено: '+f.restored.map(esc).join(', ');
    let extra='';
    if(typeof f.matched_count==='number'||typeof f.unmatched_count==='number'){
      extra=' · найдено: '+(f.matched_count||0)+', не найдено: '+(f.unmatched_count||0); }
    const warn=(f.warnings&&f.warnings.length)? '<div class="muted">'+f.warnings.map(esc).join('; ')+'</div>':'';
    const st=f.ok? '<span class="ok">готово</span>'+extra
      : '<span class="err">'+esc(f.kind||'error')+': '+esc(f.error||'')+'</span>'+
        (f.hint? '<div class="muted">'+esc(f.hint)+'</div>':'');
    return '<tr><td>'+esc(f.file||'')+'</td><td class="muted">'+esc(f.out_path||'—')+
      '</td><td class="muted">'+toks+'</td><td>'+st+warn+'</td></tr>';
  }).join('');
  h+='</tbody></table>';
  r.innerHTML=h;
}
function initTab(){
  let t='search';
  try{ t=localStorage.getItem('hds.tab')||'search'; }catch(e){}
  const known=['search','transcribe','anonymize'];
  showTab(known.indexOf(t)>=0? t : 'search');
  loadWatchDaemon();
}
initTab();
refresh(); setInterval(refresh, 3000); loadDiag(); loadTree(); loadCfg();
loadChatModels(); loadMcp(); loadServices();
</script></body></html>
"#;

/// HTML страницы **пользователя** (R4.3, `PLAN_TRANSCRIBE_SERVICE.md`): три
/// блока — «Поиск» (выдача фильтруется проектами токена), «Установка клиента»
/// (логистика M9: скрипт-бутстрап + пакет) и «Транскрипты и спикеры» (задача UI
/// транскрибации). Открывается по `/ui/user/`; в server-режиме пускает
/// `ui_user_gate` — любой валидный токен (`?token=…` или вход по логину/паролю).
///
/// Страница самодостаточна (одна HTML, без внешних ресурсов) — как [`PAGE`].
pub const PAGE_USER: &str = r#"<!doctype html>
<html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Hermes Disk Search — кабинет пользователя</title>
<style>
 body{font:14px/1.5 system-ui,Segoe UI,Arial;margin:0;background:#0f1720;color:#e6edf3}
 header{padding:14px 20px;background:#111c28;border-bottom:1px solid #24384f;display:flex;justify-content:space-between;align-items:center;gap:10px;flex-wrap:wrap}
 h1{font-size:17px;margin:0}
 main{max-width:880px;margin:0 auto;padding:16px 20px}
 section{background:#131f2b;border:1px solid #24384f;border-radius:10px;padding:14px 16px;margin:14px 0}
 h2{font-size:15px;margin:0 0 10px;color:#9fd0ff}
 input,button{font:inherit;padding:8px 10px;border-radius:8px;border:1px solid #2a3a55;background:#0d1620;color:#e6edf3}
 input{min-width:220px}button{cursor:pointer;background:#1c2f44}
 button:hover{background:#25405e}button:disabled{opacity:.5;cursor:default}
 .row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
 pre{white-space:pre-wrap;word-break:break-word;background:#0d1620;border:1px solid #24384f;border-radius:8px;padding:10px;margin:10px 0 0}
 .muted{color:#8aa0b6}.ok{color:#6ee7a8}.err{color:#ff9a9a}
 .loc{color:#9fd0ff;word-break:break-all}
 .res{border-top:1px solid #24384f;padding:8px 0}
 table.tr{width:100%;border-collapse:collapse}
 table.tr th,table.tr td{border-bottom:1px solid #24384f;padding:6px 8px;text-align:left;vertical-align:top}
 table.tr tbody tr{cursor:pointer}
 table.tr tbody tr:hover{background:#17273a}
 code{background:#0d1620;padding:1px 4px;border-radius:4px}
 a{color:#9fd0ff}
</style></head><body>
<header>
 <h1>Hermes Disk Search — кабинет пользователя</h1>
 <div class="row"><span id="user-who" class="muted">…</span>
  <button onclick="logout()">Выйти</button></div>
</header>
<main>
 <section id="user-login">
  <h2>Вход</h2>
  <div class="row">
   <input id="login-user" placeholder="логин" autocomplete="username">
   <input id="login-pass" type="password" placeholder="пароль" autocomplete="current-password">
   <button onclick="doLogin()">Войти</button>
   <span id="login-msg" class="muted"></span></div>
  <div class="muted">Токен хранится только в этом браузере (localStorage). Если админ выдал токен
   (<code>hds token new</code>), страницу можно открыть как <code>?token=…</code>.</div>
 </section>

 <section id="user-search">
  <h2>Поиск по файлам</h2>
  <div class="row">
   <input id="q" placeholder="имя, тема или содержимое" style="min-width:400px">
   <button onclick="doSearch()">Искать</button>
   <span id="q-msg" class="muted"></span></div>
  <div id="q-res"></div>
 </section>

 <section id="user-install">
  <h2>Установка клиента (Cline)</h2>
  <div class="row"><button onclick="loadClient()">Обновить данные</button>
   <span id="install-ver" class="muted">…</span></div>
  <div id="install-cmd"></div>
  <div class="muted">Скрипт скачивается <b>без токена</b>, токен передаётся аргументом: скачайте
   файл и выполните команду ниже. После установки перезапустите Cline Desktop.</div>
 </section>

 <section id="user-speakers">
  <h2>Транскрипты и спикеры</h2>
  <div class="row"><button onclick="loadTr()">Обновить список</button>
   <span id="tr-dir" class="muted"></span></div>
  <table class="tr"><thead><tr><th>Файл</th><th>Размер</th><th>Метки</th><th>Имена</th></tr></thead>
   <tbody id="tr-rows"></tbody></table>
  <div class="row" style="margin-top:8px">
   <button id="tr-open-names" onclick="openNames()" disabled>Присвоить имена спикерам</button>
   <span id="tr-msg" class="muted"></span></div>
  <pre id="tr-prev" class="muted">выберите файл в списке</pre>
  <div id="names-box" hidden>
   <div class="muted">Пустые поля оставляют метку как есть — подставляются только введённые имена.</div>
   <table class="tr"><tbody id="names-rows"></tbody></table>
   <div class="row" style="margin-top:8px"><button onclick="applyNames()">Применить</button>
    <span id="names-msg" class="muted"></span></div>
  </div>
 </section>
</main>
<script>
'use strict';
// Токен: из localStorage или ?token= (как на админской странице). Только это
// хранилище — никаких cookie: запросы идут с заголовком Authorization.
const UT=(function(){
  try{ const t=new URLSearchParams(location.search).get('token');
    if(t){ localStorage.setItem('hds.token',t); history.replaceState(null,'',location.pathname); } }catch(e){}
  try{ return localStorage.getItem('hds.token')||''; }catch(e){ return ''; }
})();
function authH(){ return UT? {'Authorization':'Bearer '+UT} : {}; }
function esc(s){ return (''+s).replace(/[&<>"']/g, function(c){
  return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]; }); }
async function jget(u){
  const r=await fetch(u,{headers:authH()});
  if(r.status===401){ throw new Error('нужен вход (401)'); }
  if(r.status===403){ throw new Error('раздел только для админа (403)'); }
  return await r.json();
}
async function jpost(u,body){
  const h=Object.assign({'Content-Type':'application/json','X-HDS-UI':'1'}, authH());
  const r=await fetch(u,{method:'POST',headers:h,body:JSON.stringify(body||{})});
  return await r.json();
}
function logout(){ try{ localStorage.removeItem('hds.token'); }catch(e){} location.reload(); }
async function doLogin(){
  const m=document.getElementById('login-msg'); m.textContent='…';
  const login=document.getElementById('login-user').value.trim();
  const password=document.getElementById('login-pass').value;
  try{ const r=await jpost('/api/auth/login',{login:login,password:password});
    if(r.token){ try{ localStorage.setItem('hds.token',r.token); }catch(e){}
      m.textContent=''; location.reload(); return; }
    m.textContent='ошибка: '+(r.error||'не удалось войти')+(r.hint? ' · '+r.hint : '');
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function loadMe(){
  const el=document.getElementById('user-who');
  try{ const j=await jget('/api/user/me');
    if(j.auth){
      el.innerHTML='<b>'+esc(j.user)+'</b>'+(j.admin? ' <span class="ok">админ</span>' : '')+
        ' · проекты: '+esc((j.projects||[]).join(', ')||'—');
    } else { el.textContent='локальный доступ (без токена)'; }
    document.getElementById('user-login').style.display=j.auth? 'none':'block';
  }catch(e){ el.innerHTML='<span class="err">'+esc(''+e)+'</span>'; }
}
async function doSearch(){
  const m=document.getElementById('q-msg'), b=document.getElementById('q-res');
  const q=document.getElementById('q').value.trim();
  if(!q){ m.textContent='введите запрос'; return; }
  m.textContent='ищу…'; b.innerHTML='';
  try{ const j=await jget('/api/search?q='+encodeURIComponent(q)+'&limit=10');
    if(j.error){ m.textContent='ошибка: '+j.error; return; }
    m.textContent='найдено: '+j.length;
    b.innerHTML=j.length? j.map(function(r){
      return '<div class="res"><div>'+esc(r.kind||'')+' · '+esc(r.ext||'')+
        '</div><div class="loc">'+esc(r.location||r.path||'')+'</div>'+
        '<div>'+esc(r.snippet||r.text||'')+'</div></div>'; }).join('')
      : '<div class="muted">ничего не найдено (попробуйте другие формулировки)</div>';
  }catch(e){ m.textContent='ошибка: '+e; }
}
async function loadClient(){
  const v=document.getElementById('install-ver'), c=document.getElementById('install-cmd');
  v.textContent='…';
  try{ const j=await jget('/api/user/client');
    const origin=location.origin;
    const tok=UT||'<TOKEN>';
    const ps1=origin+(j.routes.install_ps1||'/mcp/client-install.ps1');
    const sh=origin+(j.routes.install_sh||'/mcp/client-install.sh');
    const have=(j.packages_available||[]).length;
    v.textContent='версия сервера: '+esc(j.version||'?')+' · платформа сервера: '+
      esc(j.platform||'?')+' · клиентских пакетов на сервере: '+have;
    c.innerHTML='<div class="res"><b>Windows</b><pre>'+
      esc('iwr -UseBasicParsing '+ps1+' -OutFile $env:TEMP\\client-install.ps1\n'+
          'powershell -ExecutionPolicy Bypass -File $env:TEMP\\client-install.ps1 -Token '+tok+
          ' -ServerUrl '+origin)+'</pre></div>'+
      '<div class="res"><b>macOS</b><pre>'+
      esc('curl -f '+sh+' -o client-install.sh && bash client-install.sh --token '+tok+
          ' --server-url '+origin)+'</pre></div>'+
      '<div class="muted">MCP на сервере: '+esc(j.server.address||'')+
      ' · проверка версии: <a href="'+esc(origin+(j.routes.health||'/health'))+
      '" target="_blank" rel="noreferrer">/health</a> (поле version — сравните с версией клиента)</div>';
  }catch(e){ v.innerHTML='<span class="err">'+esc('ошибка: '+e)+'</span>'; }
}
let trFile=null, trData=null;
function fmtSize(n){ n=n||0; return n>1048576? (n/1048576).toFixed(1)+' МБ' : (n>1024? Math.round(n/1024)+' КБ' : n+' Б'); }
async function loadTr(){
  const tb=document.getElementById('tr-rows'), d=document.getElementById('tr-dir');
  tb.innerHTML='<tr><td colspan=4 class="muted">…</td></tr>';
  try{ const j=await jget('/api/transcribe/list');
    d.textContent=(j.dir||'папка не задана')+(j.error? ' · '+j.error : '');
    const f=j.files||[];
    if(!f.length){ tb.innerHTML='<tr><td colspan=4 class="muted">нет файлов</td></tr>'; return; }
    tb.innerHTML=f.map(function(x){
      return '<tr data-name="'+esc(x.name)+'"><td>'+esc(x.name)+'</td><td class="muted">'+fmtSize(x.size)+
        '</td><td class="muted">'+esc((x.speakers||[]).join(', '))+'</td><td>'+
        (x.renamed? '<span class="ok">имена присвоены</span>' : '<span class="muted">не присвоены</span>')+
        '</td></tr>'; }).join('');
    Array.prototype.forEach.call(tb.querySelectorAll('tr'), function(r){
      r.onclick=function(){ openTr(r.getAttribute('data-name')); }; });
  }catch(e){ tb.innerHTML='<tr><td colspan=4 class="err">ошибка: '+esc(''+e)+'</td></tr>'; }
}
async function openTr(name){
  const p=document.getElementById('tr-prev');
  trFile=name; trData=null; p.textContent='…';
  document.getElementById('tr-msg').textContent=name;
  document.getElementById('tr-open-names').disabled=true;
  try{ const j=await jget('/api/transcribe/file?name='+encodeURIComponent(name));
    if(j.error){ p.textContent='ошибка: '+j.error; return; }
    trData=j; p.textContent=j.text||'';
    document.getElementById('tr-open-names').disabled=false;
  }catch(e){ p.textContent='ошибка: '+e; }
}
function openNames(){
  if(!trData){ document.getElementById('tr-msg').textContent='сначала выберите файл'; return; }
  const saved=trData.saved||{}, spk=trData.speakers||[];
  const rows=spk.map(function(s){
    return '<tr><td><code>'+esc(s)+'</code></td><td><input data-sp="'+esc(s)+'" value="'+esc(saved[s]||'')+
      '" placeholder="имя (пусто — оставить)"></td></tr>'; }).join('');
  document.getElementById('names-rows').innerHTML=rows||
    '<tr><td colspan=2 class="muted">метки спикеров не найдены</td></tr>';
  document.getElementById('names-box').hidden=false;
  document.getElementById('names-msg').textContent='';
}
async function applyNames(){
  if(!trFile){ return; }
  const m=document.getElementById('names-msg'); m.textContent='…';
  const names={};
  Array.prototype.forEach.call(document.querySelectorAll('#names-rows input[data-sp]'), function(i){
    names[i.getAttribute('data-sp')]=i.value; });
  try{ const r=await jpost('/api/transcribe/speakers',{name:trFile,names:names});
    m.textContent=(r.ok? '' : 'ошибка: ')+(r.msg||r.error||'ok');
    if(r.ok){ await loadTr(); await openTr(trFile); openNames(); }
  }catch(e){ m.textContent='ошибка: '+e; }
}
loadMe(); loadClient(); loadTr();

</script></body></html>
"#;
