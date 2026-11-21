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
  <button data-tab="agent" onclick="showTab('agent')">Агент</button>
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

 <div id="tab-agent" hidden>
  <section><h2>Проекты</h2>
   <div class="row"><button onclick="loadAgentProjects()">Обновить</button>
    <span id="agmsg" class="muted"></span></div>
   <table class="tr"><thead><tr><th>Проект</th><th>Папка</th><th>Файлов</th><th>Ждут решения</th><th></th></tr></thead>
    <tbody id="agrows"></tbody></table>
   <div class="muted" id="aghint">Выберите проект — ниже состояние зеркала и файлы.</div></section>

  <section><h2>Реплика</h2>
   <div class="row"><textarea id="agtext" rows="4" style="width:100%;min-width:520px"
     placeholder="Подготовь письмо Иванову Ивану о переносе встречи"></textarea></div>
   <div class="row" style="margin-top:6px">
    <button onclick="agentPreview()">Что увидит модель</button>
    <button onclick="agentHide()">Скрыть ещё…</button>
    <button onclick="agentKeep()">Не скрывать</button>
    <button onclick="agentSend()">Отправить</button>
    <button onclick="agentStop()">Стоп</button>
    <span id="agmsg2" class="muted"></span></div>
   <div class="row" style="margin-top:6px"><span class="muted" style="min-width:120px">Вложить файл</span>
    <input id="agattach" placeholder="C:\\Проекты\\Пилот\\ТЗ.docx" style="min-width:420px">
    <button onclick="agentAttach()">Вложить файл…</button></div>
   <div class="muted" style="margin-top:8px">Что увидит модель (метки вместо значений).
    Отправляется исходный текст — предпросмотр нужен, чтобы проверить замены до отправки.</div>
   <pre id="agpreview" style="white-space:pre-wrap">…</pre>
   <div id="agpaths" class="muted"></div>
   <div class="row" style="margin-top:10px">
    <label class="muted"><input type="checkbox" id="agraw" onchange="agentRawToggle()" style="min-width:0">
     как видит модель (метки)</label>
    <span id="agstatus" class="muted"></span></div>
   <ul id="agfeed" style="list-style:none;margin:8px 0 0;padding:0;max-height:420px;overflow:auto"></ul>
   <div id="agask" hidden style="margin-top:10px;border:1px solid #3a5a80;border-radius:10px;padding:10px">
    <div><b>Вопрос агента:</b> <span id="agaskq"></span></div>
    <div class="row" style="margin-top:6px">
     <textarea id="agasktext" rows="2" style="min-width:420px"
      placeholder="Ответ уйдёт агенту; значения на выходе заменит фильтр"></textarea></div>
    <div class="row" style="margin-top:6px"><button onclick="agentAnswer()">Ответить</button>
     <span id="agaskmsg" class="muted"></span></div></div>
   <div id="agredact" hidden style="margin-top:10px;border:1px solid #8a7a2a;border-radius:10px;padding:10px">
    <div><b>Фильтр срабатывал:</b> <span id="agredactsum" class="muted">…</span></div>
    <div id="agredactlast" class="muted" style="margin-top:4px"></div>
    <div class="row" style="margin-top:6px">
     <button onclick="agentClearHistory()">Очистить историю…</button>
     <button onclick="agentRedactReset()">Сбросить сводку</button>
     <span id="agredactmsg" class="muted"></span></div>
    <div class="muted" style="margin-top:4px">Срабатывание фильтра значит, что значение уже лежит
     в истории агента: NER пропустил его при анонимизации файла или реплики. Каждый запрос фильтр
     чинит, а «Очистить историю» убирает «грязный» хвост с диска.</div></div></section>

  <section><h2>Состояние проекта</h2>
   <div id="agstate" class="muted">…</div>
   <pre id="agjournal" class="muted" style="max-height:260px;overflow:auto">…</pre></section>

  <section><h2>Результаты</h2>
   <div class="row"><button onclick="loadAgentPending()">Обновить</button>
    <span id="agresmsg" class="muted"></span></div>
   <table class="tr"><thead><tr><th>Вариант</th><th>Статус</th><th>Отчёт</th>
     <th>Действия</th></tr></thead><tbody id="agresults"></tbody></table>
   <div id="agbackups" hidden style="margin-top:10px;border:1px solid #3a5a80;border-radius:10px;padding:10px">
    <div class="row"><b id="agbkpTitle">Копии</b>
     <button onclick="agentBackupsClose()">Закрыть</button>
     <span id="agbkpmsg" class="muted"></span></div>
    <div class="row" style="margin-top:6px">
     <label class="muted">Версия: <select id="agbkpStamp"></select></label>
     <button onclick="agentRestore()">Вернуть…</button>
     <button onclick="agentRevealCopy()">Показать в Проводнике…</button>
     <button onclick="agentCopySelectedCopy()">Копировать путь</button></div>
    <div class="muted" style="margin-top:4px">«Вернуть» заменит оригинал выбранной копией и
     пересоберёт образ; сами копии остаются в <code>.hds-agent\backup</code>. «Показать в
     Проводнике» — отдельное действие с подтверждением: открывается Проводник с выделением
     файла, документы и приложения не запускаются.</div></div>
   <div id="agcompare" hidden style="margin-top:10px;border:1px solid #3a5a80;border-radius:10px;padding:10px">
    <div class="row"><b id="agcmpTitle"></b>
     <button onclick="agentCompareClose()">Закрыть</button>
     <span id="agcmpmsg" class="muted"></span></div>
    <div id="agcmpMeta" class="muted" style="margin-top:6px"></div>
    <div id="agcmpReport" class="muted" style="margin-top:6px"></div>
    <pre id="agcmpDiff" style="max-height:320px;overflow:auto;white-space:pre-wrap"></pre></div>
   <div class="muted" style="margin-top:6px">«Принять» заменит оригинал вариантом (копия прежней
    версии — в <code>.hds-agent\backup</code>); «Отклонить» вернёт образ к оригиналу. Если оригинал
    открыт в Word/Excel, сервер ответит «файл занят», а вариант останется на месте.</div></section>

  <section><h2>Файлы проекта</h2>
   <div class="row"><button onclick="loadAgentFiles()">Обновить</button>
    <span id="agfilesmsg" class="muted"></span></div>
   <table class="tr"><thead><tr><th>Оригинал</th><th>Образ</th><th>Вид</th><th>Статус</th>
     <th>Действия</th></tr></thead>
    <tbody id="agfiles"></tbody></table>
   <div class="muted" style="margin-top:4px">«Копии…» — прежние версии оригинала;
    «Пересоздать образ» доступно только для статуса <code>orphan</code> (образ удалён или
    оригинал заменён агентом). «Показать в Проводнике» открывает Проводник с выделением
    файла — с отдельным подтверждением.</div></section>

  <section><h2>Метки проекта</h2>
   <div class="row"><button onclick="loadAgentLabels()">Обновить</button>
    <span id="aglabelsmsg" class="muted"></span></div>
   <table class="tr"><thead><tr><th></th><th>Метка</th><th>Значение (формы)</th>
    <th>Где встречается</th></tr></thead><tbody id="aglabels"></tbody></table>
   <div class="row" style="margin-top:6px">
    <button onclick="agentMerge()">Объединить выбранные</button>
    <span class="muted">старшая — метка с наибольшим числом файлов</span></div>
   <div class="muted" style="margin-top:4px">«Где встречается» — по тексту образов (тело документа,
    без заголовков docx). «Разъединить» отменяет склейку, записанную в журнале (после A5.5);
    склейки до A5.5 необратимы.</div>
   <table class="tr"><thead><tr><th>Когда</th><th>Оставлено</th><th>Склеено</th><th></th></tr></thead>
    <tbody id="agmerges"></tbody></table></section>

  <section><h2>Настройки агента</h2>
   <div class="row"><button onclick="loadAgentSettings()">Обновить</button>
    <span id="agsetmsg" class="muted"></span></div>
   <table class="tr"><thead><tr><th>Параметр</th><th>Значение</th><th></th></tr></thead>
    <tbody id="agsettings"></tbody></table>
   <div class="muted" style="margin-top:6px">Провайдер и модель проекта — в
    <code>projects.json</code>; ключ — только в <code>data\agent\secrets.json</code> и показывается
    признаком «задан/нет»; пороги фильтра и MCP — в <code>config.yaml</code> (ключей там нет).
    Смена ключа, провайдера, модели или фильтра применится следующим ходом — сессия откроется заново.</div>
   <div class="row" style="margin-top:8px"><button onclick="agentClearHistory()">Очистить историю…</button>
    <span id="aghistmsg" class="muted"></span></div>
   <div class="muted">«Очистить историю»: остановить сессию, заменить значения в файлах истории
    метками (рядом останутся бэкапы) и возобновить сессию. Автопроверка истории перед завершением
    сессии — A6.</div></section>

  <section><h2>Новый проект</h2>
   <div class="row"><span class="muted" style="min-width:120px">Имя</span>
    <input id="agname" placeholder="Пилот" style="min-width:260px"></div>
   <div class="row" style="margin-top:6px"><span class="muted" style="min-width:120px">Папка</span>
    <input id="agpath" placeholder="D:\Проекты\Внедрение ERP" style="min-width:420px"></div>
   <div class="row" style="margin-top:8px"><button onclick="agentAdd()">Создать проект</button>
    <span id="agaddmsg" class="muted"></span></div>
   <div class="muted">Идентификатор (P0001, P0002, …) выдаётся автоматически. Папка проекта — ваша,
    вне <code>agent.root</code>; после создания запустите зеркало:
    <code>hds agent sync --project …</code>.</div></section>
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
// --- Агент (A5.1): проекты, состояние зеркала, файлы реестра ---
let AG_ID='';
async function loadAgentProjects(){
  const m=document.getElementById('agmsg'); m.textContent='…';
  try{
    const j=await jget('/api/agent/projects');
    if(j.error){
      m.innerHTML='<span class="err">'+esc(j.error)+'</span>';
      if(j.error.indexOf('серверном профиле')>=0){
        const b=document.querySelector('nav.tabs button[data-tab="agent"]');
        if(b) b.style.display='none';
      }
      return;
    }
    m.textContent=(j.enabled? 'зона включена':'зона выключена (agent.enabled: false)')+
      (j.root? ' · '+j.root : ' · agent.root не задан');
    const list=j.projects||[];
    document.getElementById('agrows').innerHTML=list.map(p=>
      '<tr data-id="'+esc(p.id)+'" onclick="selectAgentProject(this)">'+
      '<td><b>'+esc(p.id)+'</b> '+esc(p.name)+'</td><td class="muted">'+esc(p.path)+'</td>'+
      '<td>'+p.files+'</td><td>'+(p.pending? '<b>'+p.pending+'</b>':'0')+'</td>'+
      '<td><button onclick="agentRemoveProject(event,\''+esc(p.id)+'\')">Удалить…</button></td></tr>').join('')||
      '<tr><td colspan="5" class="muted">проектов пока нет — создайте ниже</td></tr>';
    if(!AG_ID && list.length) selectAgentProject(null, list[0].id);
  }catch(e){ m.textContent='ошибка: '+e; }
}
function selectAgentProject(tr, id){
  AG_ID=id || (tr&&tr.getAttribute('data-id')) || '';
  Array.prototype.forEach.call(document.querySelectorAll('#agrows tr'), function(r){
    r.classList.toggle('sel', r.getAttribute('data-id')===AG_ID); });
  agentStreamClose();
  agentBackupsClose();
  AG_PREVIEW=null; AG_SEQ=0;
  agReset();
  document.getElementById('agpreview').textContent='…';
  document.getElementById('agpaths').textContent='';
  agStatus('');
  loadAgentState(); loadAgentFiles(); loadAgentPending(); loadAgentLabels(); loadAgentSettings();
  agentStreamOpen();
}
// --- Реплика (A5.2): предпросмотр, метки, вложения, отправка ---
let AG_PREVIEW=null, AG_POLL=null; const NL=String.fromCharCode(10);
function agentMsg(text, isErr){
  const m=document.getElementById('agmsg2');
  if(isErr) m.innerHTML='<span class="err">'+esc(text)+'</span>'; else m.textContent=text;
}
function agentText(){ return document.getElementById('agtext').value; }
function agentNeedProject(){ if(!AG_ID){ agentMsg('сначала выберите проект', true); return false; } return true; }
// Подсветка меток — только представление: токены берём из ответа сервера,
// своих регулярок меток в UI нет (AG-1).
function agentHighlight(text, labels){
  let html=esc(text);
  (labels||[]).slice().sort(function(a,b){ return (b.token||'').length-(a.token||'').length; })
    .forEach(function(l){ const t=esc(l.token||''); if(t) html=html.split(t).join('<mark>'+t+'</mark>'); });
  return html;
}
async function agentPreview(){
  if(!agentNeedProject()) return null;
  const text=agentText();
  if(!text.trim()){ agentMsg('введите реплику', true); return null; }
  agentMsg('…');
  try{
    const j=await jpost('/api/agent/preview',{project:AG_ID,text:text});
    if(j.error){ agentMsg(j.error, true); return null; }
    AG_PREVIEW={src:text};
    document.getElementById('agpreview').innerHTML=agentHighlight(j.text, j.labels);
    document.getElementById('agpaths').innerHTML=(j.paths||[]).map(function(p){
      return 'образ ' + esc(p.anon||p.to); }).join('<br>');
    const marks=(j.labels||[]).map(function(l){ return l.token; }).join(', ');
    agentMsg((j.changed? 'замены есть':'замен нет')+' · метки: '+(marks||'нет')+
      ((j.warnings||[]).length? ' · предупреждения: '+j.warnings.join('; '):''));
    return j;
  }catch(e){ agentMsg('ошибка: '+e, true); return null; }
}
async function agentHide(){
  if(!agentNeedProject()) return;
  const ta=document.getElementById('agtext');
  let value=ta.value.substring(ta.selectionStart, ta.selectionEnd).trim();
  if(!value) value=(window.prompt('Что скрыть? (значение целиком)')||'').trim();
  if(!value){ agentMsg('выделите значение в реплике', true); return; }
  try{
    const j=await jpost('/api/agent/label',{project:AG_ID,value:value});
    if(j.error){ agentMsg(j.error, true); return; }
    if(j.kept) agentMsg('значение уже в списке «не скрывать» — метка не выдана');
    else if(!j.label) agentMsg('не удалось разобрать значение — выделите другое', true);
    else agentMsg('метка '+j.label+(j.confident? '':' (падеж проверить)'));
    await agentPreview();
  }catch(e){ agentMsg('ошибка: '+e, true); }
}
async function agentKeep(){
  if(!agentNeedProject()) return;
  const ta=document.getElementById('agtext');
  let value=ta.value.substring(ta.selectionStart, ta.selectionEnd).trim();
  if(!value) value=(window.prompt('Что не скрывать? (значение целиком)')||'').trim();
  if(!value){ agentMsg('выделите значение в реплике', true); return; }
  try{
    const j=await jpost('/api/agent/keep',{project:AG_ID,value:value});
    if(j.error){ agentMsg(j.error, true); return; }
    agentMsg(j.added? 'добавлено в базу «не скрывать» (действует во всех проектах)':'уже было в базе');
    await agentPreview();
  }catch(e){ agentMsg('ошибка: '+e, true); }
}
async function agentAttach(){
  if(!agentNeedProject()) return;
  const path=document.getElementById('agattach').value.trim();
  if(!path){ agentMsg('укажите путь к файлу', true); return; }
  agentMsg('…');
  try{
    const j=await jpost('/api/agent/attach',{project:AG_ID,path:path});
    if(j.error){ agentMsg(j.error, true); return; }
    const ta=document.getElementById('agtext');
    ta.value=(ta.value.trim()? ta.value.trimEnd()+NL:'')+(j.anon||j.path)+NL;
    agentMsg((j.kind==='image'? 'образ файла: ':'вложение: ')+j.path);
  }catch(e){ agentMsg('ошибка: '+e, true); }
}
async function agentSend(){
  if(!agentNeedProject()) return;
  const text=agentText();
  if(!text.trim()){ agentMsg('введите реплику', true); return; }
  if(!AG_PREVIEW || AG_PREVIEW.src!==text){
    await agentPreview();
    agentMsg('проверьте «Что увидит модель» и нажмите «Отправить» ещё раз');
    return;
  }
  agentMsg('…отправка');
  // Лента — только текущий ход: AG_SEQ (номер до хода) сохраняем, DOM чистим.
  agentStreamClose(); agReset();
  try{
    const j=await jpost('/api/agent/message',{project:AG_ID,text:text});
    if(j.error){ agentMsg(j.error, true); agentStreamOpen(); return; }
    AG_BUSY=true;
    agentMsg('ход принят · сессия '+(j.session_id||'-'));
    // Поток откроется с after=AG_SEQ: prompt и весь ход уже в кольце.
    agentStreamOpen();
    if(!AG_ES) agentPoll();
  }catch(e){ agentMsg('ошибка: '+e, true); }
}
async function agentStop(){
  if(!agentNeedProject()) return;
  try{
    const j=await jpost('/api/agent/abort',{project:AG_ID});
    if(j.error){ agentMsg(j.error, true); return; }
    agentMsg('прерывание отправлено');
    if(!AG_ES) agentPoll();
  }catch(e){ agentMsg('ошибка: '+e, true); }
}
// --- Лента событий (A5.3): SSE, склейка дельт, переключатель «как видит модель» ---
let AG_SEQ=0, AG_ES=null, AG_RAW=false, AG_BUSY=false;
let AG_EVENTS=[], AG_TEXT=null, AG_REASON=null, AG_ASK=null;
const AG_MAX=500, AG_CLIP=20000, AG_INPUT_CLIP=500;
function agFeed(){ return document.getElementById('agfeed'); }
function agStatus(s){ document.getElementById('agstatus').textContent=s||''; }
function agLi(color){
  const li=document.createElement('li');
  li.style.margin='6px 0';
  li.style.paddingLeft='8px';
  if(color) li.style.borderLeft='3px solid '+color;
  return li;
}
function agAppend(li){
  const box=agFeed();
  if(!box) return;
  box.appendChild(li);
  // Не больше 500 узлов в DOM — как кольцо на сервере.
  while(box.children.length>AG_MAX){ box.removeChild(box.firstChild); }
  box.scrollTop=box.scrollHeight;
}
function agLine(text, cls){
  const li=agLi('');
  li.innerHTML='<span class="'+((cls&&cls!=='muted')? cls:'muted')+'">'+esc(text)+'</span>';
  agAppend(li);
}
function agReset(){
  AG_EVENTS=[]; AG_TEXT=null; AG_REASON=null; AG_BUSY=false;
  const box=agFeed(); if(box) box.innerHTML='';
  agAskHide();
  agentRedactReset();
}
// Принять событие: номер для переподключения, копия для перерисовки, показ.
function agOnItem(ev){
  if(!ev) return;
  if(typeof ev.seq==='number' && ev.seq>AG_SEQ) AG_SEQ=ev.seq;
  AG_EVENTS.push(ev);
  while(AG_EVENTS.length>AG_MAX) AG_EVENTS.shift();
  agApply(ev);
}
function agApply(ev){
  const kind=ev.kind||'';
  const raw=ev.raw||null;
  // «Как видит модель» — сырое событие (метки); у prompt raw нет.
  const t=(AG_RAW && raw)? raw : (ev.text||{});
  if(kind==='agent'){ agAgentEvent(t); return; }
  if(kind==='prompt'){
    const li=agLi('#3a5a80');
    li.innerHTML='<div class="muted">→ модели'+((AG_RAW && raw)?' (метки)':' (подтверждённая реплика)')+
      ':</div><div style="white-space:pre-wrap">'+esc(t.text||'')+'</div>';
    agAppend(li); return;
  }
  if(kind==='session'){
    agLine('сессия '+(t.session||'-')+(t.resumed?' (возобновлена)':''),'muted'); return;
  }
  if(kind==='redacted'){ agRedacted(t); return; }
  if(kind==='ask'){ agAsk(t); return; }
  if(kind==='ask_timeout'){
    agAskHide();
    agLine('вопрос без ответа (таймаут '+(t.timeout_sec||'?')+' с) — агент продолжил','err');
    return;
  }
  if(kind==='ask_cancelled'){
    agAskHide(); agLine('вопрос снят: '+(t.reason||'ход остановлен'),'muted'); return;
  }
  if(kind==='turn_end'){ agTurnEnd(t); return; }
  if(kind==='done'){
    agLine('ответ воркера: '+(t.ok?'ok':'ошибка'+(t.error?': '+t.error:'')), t.ok?'ok':'err');
    return;
  }
  if(kind==='error'){ agLine('ошибка: '+(t.error||''),'err'); return; }
  if(kind==='notice'){ agLine(t.text||'','muted'); return; }
  agLine(kind,'muted');
}
function agAgentEvent(t){
  const k=t.kind||'';
  if(k==='reasoning'||k==='reasoning_delta'){
    AG_TEXT=null;
    if(!AG_REASON){
      const li=agLi('#3a5a80');
      const det=document.createElement('details');
      det.innerHTML='<summary class="muted">рассуждение</summary>';
      const body=document.createElement('div');
      body.className='muted';
      body.style.whiteSpace='pre-wrap';
      det.appendChild(body);
      li.appendChild(det);
      agAppend(li);
      AG_REASON=body;
    }
    AG_REASON.textContent=(k==='reasoning')? String(t.text||'')
      : AG_REASON.textContent+String(t.text||'');
    return;
  }
  if(k==='text'||k==='text_delta'){
    AG_REASON=null;
    if(!AG_TEXT){
      const li=agLi('#3a7a5a');
      const body=document.createElement('div');
      body.style.whiteSpace='pre-wrap';
      li.appendChild(body);
      agAppend(li);
      AG_TEXT=body;
    }
    // `text` заменяет накопленные дельты, `text_delta` дописывается.
    AG_TEXT.textContent=(k==='text')? String(t.text||'')
      : AG_TEXT.textContent+String(t.text||'');
    return;
  }
  if(k==='tool_use'){
    AG_TEXT=null; AG_REASON=null;
    const input=(typeof t.input==='string')? t.input : JSON.stringify(t.input===undefined? null : t.input);
    const li=agLi('#4a4a6a');
    li.innerHTML='<span class="muted">инструмент</span> <b>'+esc(t.tool||'?')+
      '</b> ← <span class="muted">'+esc(String(input||'').slice(0,AG_INPUT_CLIP))+'</span>';
    // «Открыть» = показать путь и скопировать (В-A5-7; процессы UI не
    // запускает, кроме отдельного «Показать в Проводнике», В-A6-7).
    const path=(t.input && typeof t.input==='object' && t.input.path)? String(t.input.path) : '';
    if(path){
      const a=document.createElement('a');
      a.href='#';
      a.className='loc';
      a.textContent=' скопировать путь';
      a.onclick=function(ev){
        if(ev && ev.preventDefault) ev.preventDefault();
        agentCopyPath(path);
        return false;
      };
      li.appendChild(a);
    }
    agAppend(li);
    return;
  }
  if(k==='tool_result'){
    const out=(typeof t.output==='string')? t.output : JSON.stringify(t.output===undefined? null : t.output);
    const text=String(out||'');
    const li=agLi(t.error? '#8a3a3a':'#4a4a6a');
    const det=document.createElement('details');
    det.innerHTML='<summary>результат <b>'+esc(t.tool||'')+'</b>'+
      (t.error? ' <span class="err">ошибка: '+esc(t.error)+'</span>':'')+
      ' <span class="muted">'+esc(text.length>200? text.slice(0,200)+'…' : text)+'</span></summary>';
    const pre=document.createElement('pre');
    pre.style.maxHeight='240px';
    pre.style.overflow='auto';
    // Потолок — как у воркера (20 000 символов), иначе лента съест память.
    pre.textContent=text.slice(0,AG_CLIP);
    det.appendChild(pre);
    li.appendChild(det);
    agAppend(li);
    return;
  }
  if(k==='usage'){
    const tokens=(t.total_input_tokens||0)+(t.total_output_tokens||0);
    agStatus('ход: '+(AG_BUSY?'идёт':'свободен')+' · токены: '+tokens);
    return;
  }
  if(k==='iteration'){ agStatus('ход: идёт · итерация '+(t.iteration||'?')); return; }
  if(k==='status'||k==='agent_done'||k==='ended'){
    agStatus('ход: '+(k==='agent_done'?'завершён':(k==='ended'?'закрыт':'идёт'))+
      (t.reason? ' · '+t.reason:''));
    return;
  }
  if(k==='notice'){ agLine(t.text||t.notice||'','muted'); return; }
  if(k==='error'){ agLine('ошибка: '+(t.text||''),'err'); return; }
  agStatus('событие: '+k);
}
function agRedacted(t){
  const hits=t.hits||[];
  // Сводка — по событиям, без значений: только метки и источник словаря.
  hits.forEach(function(h){
    AG_REDACT.total+=1;
    const label=h.label||'?';
    AG_REDACT.labels[label]=(AG_REDACT.labels[label]||0)+1;
    const src=h.source||'?';
    AG_REDACT.sources[src]=(AG_REDACT.sources[src]||0)+1;
  });
  AG_REDACT.last=hits.map(function(h){ return h.label+(h.source? ' ('+h.source+')':''); });
  agRedactRender();
  const text=AG_REDACT.last.join(', ');
  const li=agLi('#8a7a2a');
  li.innerHTML='<span class="warnbox" style="margin:0">фильтр заменил: '+esc(text||'метки')+'</span>';
  agAppend(li);
}
function agTurnEnd(t){
  const streamed=AG_TEXT? AG_TEXT.textContent.trim() : '';
  AG_TEXT=null; AG_REASON=null; AG_BUSY=false;
  const ok=t.ok!==false;
  let line='итог хода: '+(ok?'готово':'сбой');
  if(t.reason) line+=' · '+t.reason;
  if(t.error) line+=' — '+t.error;
  if(t.tools && t.tools.length) line+=' · инструменты: '+t.tools.join(', ');
  agLine(line, ok?'ok':'err');
  // Финальный текст модель могла не отдать дельтами — показываем, если его нет в ленте.
  const final=String(t.text||'').trim();
  if(ok && final && final!==streamed) agLine(final,'');
  agStatus('ход: свободен'+(t.reason? ' · '+t.reason:''));
  agAskHide();
}
function agAsk(t){
  AG_ASK={id:t.ask_id, project:AG_ID};
  document.getElementById('agaskq').textContent=t.question||'';
  document.getElementById('agasktext').value='';
  document.getElementById('agaskmsg').textContent='';
  document.getElementById('agask').hidden=false;
  const li=agLi('#8a7a2a');
  li.innerHTML='<b>вопрос агента:</b> '+esc(t.question||'');
  agAppend(li);
  try{ document.getElementById('agasktext').focus(); }catch(e){}
}
function agAskHide(){
  AG_ASK=null;
  const panel=document.getElementById('agask');
  if(panel) panel.hidden=true;
}
// Путь — в буфер обмена. UI не открывает документы приложениями (В-A5-7);
// отдельное действие «Показать в Проводнике» — A6.4 (В-A6-7).
function agentCopyPath(path){
  if(navigator.clipboard && navigator.clipboard.writeText){
    navigator.clipboard.writeText(path).then(
      function(){ agLine('путь скопирован: '+path,'ok'); },
      function(){ agLine('путь: '+path,'muted'); });
  } else {
    agLine('путь: '+path,'muted');
  }
}
async function agentAnswer(){
  if(!AG_ID) return;
  const msg=document.getElementById('agaskmsg');
  if(!AG_ASK){ msg.textContent='вопрос уже снят — ждите нового'; return; }
  const text=document.getElementById('agasktext').value.trim();
  if(!text){ msg.textContent='введите ответ'; return; }
  msg.textContent='…';
  try{
    const j=await jpost('/api/agent/answer',{project:AG_ID,ask_id:AG_ASK.id,answer:text});
    if(j.error){ msg.innerHTML='<span class="err">'+esc(j.error)+'</span>'; return; }
    agLine('ответ отправлен','ok');
    agAskHide();
  }catch(e){ msg.textContent='ошибка: '+e; }
}
// Переключатель «как видит модель»: перерисовать ленту из принятых событий.
function agentRawToggle(){
  AG_RAW=document.getElementById('agraw').checked;
  const box=agFeed(); if(box) box.innerHTML='';
  AG_TEXT=null; AG_REASON=null;
  agAskHide();
  AG_EVENTS.forEach(agApply);
}
// Одна подписка на вкладку: при смене проекта/скрытии вкладки поток закрывается.
function agentStreamClose(){
  if(AG_ES){ try{ AG_ES.close(); }catch(e){} AG_ES=null; }
}
function agentStreamOpen(){
  agentStreamClose();
  if(!AG_ID) return;
  if(typeof EventSource==='undefined'){ agentPoll(); return; }
  const url='/api/agent/events?project='+encodeURIComponent(AG_ID)+
    '&after='+AG_SEQ+'&limit='+AG_MAX;
  let es=null;
  try{ es=new EventSource(url); }catch(e){ agentPoll(); return; }
  AG_ES=es;
  es.addEventListener('state', function(e){
    try{
      const s=JSON.parse(e.data||'{}');
      AG_BUSY=!!s.busy;
      agStatus('ход: '+(s.busy?'идёт':'свободен')+' · сессия: '+(s.session||'-')+
        ' · событие: '+(s.seq||0));
    }catch(err){}
  });
  es.addEventListener('item', function(e){
    // AG_SEQ — из `id:` кадра, а не из поля seq: переподключение не теряет и не дублирует.
    const id=parseInt(e.lastEventId||'',10);
    if(!isNaN(id) && id>AG_SEQ) AG_SEQ=id;
    try{ agOnItem(JSON.parse(e.data||'{}')); }catch(err){}
  });
  // onerror: EventSource переподключится сам (сервер отдаёт retry: 1500).
}
// JSON-фолбэк: только если браузер не умеет EventSource.
async function agentPoll(){
  if(!AG_ID || AG_ES) return;
  try{
    const j=await jget('/api/agent/events?project='+encodeURIComponent(AG_ID)+'&after='+AG_SEQ);
    if(j.error) return;
    AG_BUSY=!!j.busy;
    (j.events||[]).forEach(agOnItem);
    agStatus('ход: '+(j.busy? 'идёт':'свободен')+' · сессия: '+(j.session||'-')+
      ' · последнее событие: '+(j.seq||0));
    if(AG_POLL){ clearTimeout(AG_POLL); AG_POLL=null; }
    if(j.busy) AG_POLL=setTimeout(agentPoll, 1200);
  }catch(e){}
}
document.addEventListener('visibilitychange', function(){
  if(document.hidden){ agentStreamClose(); return; }
  const tab=document.getElementById('tab-agent');
  if(AG_ID && tab && tab.style.display==='block') agentStreamOpen();
});
window.addEventListener('beforeunload', agentStreamClose);
async function loadAgentState(){
  const box=document.getElementById('agstate'), pre=document.getElementById('agjournal');
  if(!AG_ID){ box.textContent='проект не выбран'; pre.textContent=''; return; }
  try{
    const j=await jget('/api/agent/state?project='+encodeURIComponent(AG_ID));
    if(j.error){ box.innerHTML='<span class="err">'+esc(j.error)+'</span>'; pre.textContent=''; return; }
    box.innerHTML='<pre>'+esc('проект '+j.id+' · '+j.name+'\nпапка: '+j.path+
      '\nпровайдер: '+j.provider+' · '+j.model+
      '\nзона: '+(j.root||'не задана')+(j.enabled?'':' (выключена)')+
      '\nдемон зеркала: '+(j.daemon?'работает':'не запущен')+
      '\nфайлов: '+j.files+' · ждут решения: '+j.pending+
      '\nистория: '+((j.history&&j.history.uncleaned)?('не очищена — '+j.history.hint):
        (j.history&&j.history.session?'очищена':'сессии нет')))+'</pre>';
    pre.textContent=(j.journal||[]).join('\n')||'(журнал пуст)';
  }catch(e){ box.textContent='ошибка: '+e; }
}
let AG_FILES=[];
async function loadAgentFiles(){
  const tb=document.getElementById('agfiles'), m=document.getElementById('agfilesmsg');
  if(!AG_ID){ tb.innerHTML=''; m.textContent=''; AG_FILES=[]; return; }
  m.textContent='…';
  try{
    const j=await jget('/api/agent/files?project='+encodeURIComponent(AG_ID));
    if(j.error){ m.innerHTML='<span class="err">'+esc(j.error)+'</span>'; tb.innerHTML=''; AG_FILES=[]; return; }
    AG_FILES=j.files||[];
    m.textContent='файлов: '+j.count+' · ждут решения: '+j.pending;
    tb.innerHTML=AG_FILES.map(function(f,i){
      // A6.4: «Пересоздать образ» — только у orphan (у остальных образ либо
      // актуален, либо ждёт решения в pending).
      let act='<button onclick="agentBackupsForFile('+i+')">Копии…</button> '+
        '<button onclick="agentRevealRel(AG_FILES['+i+'] && AG_FILES['+i+'].orig)">Показать в Проводнике…</button>';
      if(f.status==='orphan') act+=' <button onclick="agentRebuildFile('+i+')">Пересоздать образ</button>';
      return '<tr><td>'+esc(f.orig)+'</td><td class="muted">'+esc(f.anon)+
        '</td><td>'+esc(f.kind)+'</td><td>'+esc(f.status)+'</td><td>'+act+'</td></tr>';
    }).join('')||
      '<tr><td colspan="5" class="muted">файлов нет — запустите hds agent sync</td></tr>';
  }catch(e){ m.textContent='ошибка: '+e; }
}
// --- Метки проекта (A5.5): формы, «Не скрывать», «Объединить», «Разъединить» ---
let AG_LABELS=[];
function agLabelsMsg(text, isErr){
  const m=document.getElementById('aglabelsmsg');
  if(isErr) m.innerHTML='<span class="err">'+esc(text)+'</span>'; else m.textContent=text;
}
function agLabelFiles(l){ return ((l&&l.occurrences)||[]).length; }
// Колонка «Значение (формы)»: формы — значения (зона пользователя); кнопка
// «Не скрывать» пишет в общий keep_terms (D19, действует во всех проектах).
function agLabelForms(l, i){
  const forms=(l&&l.forms)||[];
  if(!forms.length) return '<span class="muted">—</span>';
  return forms.map(function(f, j){
    return '<div>'+esc(f.value)+' <span class="muted">'+esc(f.gram||'ном')+
      (f.origin==='derived'? ' · порождённая':'')+'</span>'+
      ' <button onclick="agLabelKeep('+i+','+j+')">Не скрывать</button></div>';
  }).join('');
}
// Колонка «Где встречается»: число файлов + первые 3 пути, остальное — «ещё N».
function agLabelWhere(l){
  const occ=(l&&l.occurrences)||[];
  if(!occ.length) return '<span class="muted">в образе не встречается</span>';
  const first=occ.slice(0,3).map(function(o){
    return esc(o.rel)+' <span class="muted">·'+o.count+'</span>';
  }).join('<br>');
  const more=occ.length>3? '<div class="muted">ещё '+(occ.length-3)+'</div>':'';
  return '<div class="muted">файлов: '+occ.length+' · форм: '+((l.forms||[]).length)+
    (l.aliases&&l.aliases.length? ' · псевдонимы: '+esc(l.aliases.join(', ')):'')+'</div>'+first+more;
}
function agLabelRow(l, i){
  return '<tr><td><input type="checkbox" class="aglabelpick" data-i="'+i+'" style="min-width:0"></td>'+
    '<td><b>'+esc(l.label)+'</b> <span class="muted">'+esc(l.type||'')+'</span></td>'+
    '<td>'+agLabelForms(l, i)+'</td><td>'+agLabelWhere(l)+'</td></tr>';
}
function agMergeRow(m){
  const when=m.ts? new Date(m.ts*1000).toLocaleString():'-';
  return '<tr><td class="muted">'+esc(when)+'</td><td>'+esc(m.keep)+
    '</td><td>'+esc((m.dropped||[]).join(', '))+
    '</td><td><button onclick="agentUnmerge('+m.id+')">Разъединить</button></td></tr>';
}
async function loadAgentLabels(){
  const tb=document.getElementById('aglabels'), mg=document.getElementById('agmerges');
  if(!AG_ID){ tb.innerHTML=''; mg.innerHTML=''; agLabelsMsg(''); AG_LABELS=[]; return; }
  agLabelsMsg('…');
  try{
    const j=await jget('/api/agent/labels?project='+encodeURIComponent(AG_ID));
    if(j.error){ agLabelsMsg(j.error, true); tb.innerHTML=''; mg.innerHTML=''; AG_LABELS=[]; return; }
    AG_LABELS=j.labels||[];
    agLabelsMsg('меток: '+AG_LABELS.length+' · склеек: '+((j.merges||[]).length)+' · rev '+j.rev);
    tb.innerHTML=AG_LABELS.map(agLabelRow).join('')||
      '<tr><td colspan="4" class="muted">меток нет — запустите hds agent sync</td></tr>';
    mg.innerHTML=(j.merges||[]).map(agMergeRow).join('')||
      '<tr><td colspan="4" class="muted">склеек нет</td></tr>';
  }catch(e){ agLabelsMsg('ошибка: '+e, true); }
}
async function agLabelKeep(i, j){
  const l=AG_LABELS[i], f=l&&(l.forms||[])[j];
  if(!f){ agLabelsMsg('форма не найдена — обновите панель', true); return; }
  agLabelsMsg('…');
  try{
    const r=await jpost('/api/agent/keep', {project:AG_ID, value:f.value});
    if(r.error){ agLabelsMsg(r.error, true); return; }
    agLabelsMsg(r.added? 'добавлено в базу «не скрывать» (действует во всех проектах)':'уже было в базе');
    await loadAgentLabels();
  }catch(e){ agLabelsMsg('ошибка: '+e, true); }
}
function agLabelPicked(){
  const out=[];
  Array.prototype.forEach.call(document.querySelectorAll('#aglabels input.aglabelpick:checked'),
    function(cb){ const i=+cb.getAttribute('data-i'); if(AG_LABELS[i]) out.push(AG_LABELS[i]); });
  return out;
}
// «Объединить выбранные»: один тип, старшая — максимум файлов (В-A5.5-1).
async function agentMerge(){
  if(!AG_ID){ agLabelsMsg('сначала выберите проект', true); return; }
  const picked=agLabelPicked();
  if(picked.length<2){ agLabelsMsg('отметьте минимум две метки одного типа', true); return; }
  const type=picked[0].type||'';
  if(picked.some(function(l){ return (l.type||'')!==type; })){
    agLabelsMsg('метки разных типов склеить нельзя — отметьте метки одного типа', true); return;
  }
  let keep=picked[0];
  picked.forEach(function(l){ if(agLabelFiles(l)>agLabelFiles(keep)) keep=l; });
  const drop=picked.filter(function(l){ return l.label!==keep.label; })
    .map(function(l){ return l.label; });
  if(!drop.length){ agLabelsMsg('нужны минимум две разные метки', true); return; }
  if(!confirm('Объединить метки?'+NL+'старшая: '+keep.label+NL+'склеиваются: '+drop.join(', ')+
      NL+NL+'Образы будут пересобраны, агенту уйдёт сообщение.')) return;
  agLabelsMsg('…');
  try{
    const j=await jpost('/api/agent/labels/merge', {project:AG_ID, keep:keep.label, drop:drop});
    if(j.error){ agLabelsMsg(j.error, true); return; }
    await loadAgentLabels(); await loadAgentFiles(); await loadAgentState();
    agLabelsMsg('склеено: '+drop.join(', ')+' → '+j.keep+' · пересобрано образов: '+
      ((j.rebuilt||[]).length)+((j.skipped||[]).length? ' · пропущено: '+j.skipped.length:''));
  }catch(e){ agLabelsMsg('ошибка: '+e, true); }
}
async function agentUnmerge(mergeId){
  if(!AG_ID){ agLabelsMsg('сначала выберите проект', true); return; }
  if(!confirm('Отменить склейку №'+mergeId+'? Метки снова станут самостоятельными, '+
      'образы будут пересобраны.')) return;
  agLabelsMsg('…');
  try{
    const j=await jpost('/api/agent/labels/unmerge', {project:AG_ID, merge_id:mergeId});
    if(j.error){ agLabelsMsg(j.error, true); return; }
    await loadAgentLabels(); await loadAgentFiles(); await loadAgentState();
    agLabelsMsg('разъединено: '+(j.restored||[]).join(', ')+' · пересобрано образов: '+
      ((j.rebuilt||[]).length));
  }catch(e){ agLabelsMsg('ошибка: '+e, true); }
}
// --- Результаты (A5.4): варианты, «Сравнить», решения ---
let AG_RESULTS=[];
function agResMsg(text, isErr){
  const m=document.getElementById('agresmsg');
  if(isErr) m.innerHTML='<span class="err">'+esc(text)+'</span>'; else m.textContent=text;
}
// Колонка «Отчёт»: вердикт валидатора (TR-7.8) и число проверок.
function agReportLabel(r){
  if(!r) return '<span class="muted">нет отчёта</span>';
  return (r.verdict==='check'? '<span class="err">проверить</span>' : '<span class="ok">ок</span>')+
    ' <span class="muted">· проверок '+r.checks+'</span>';
}
// Действия по варианту; кнопки конфликта — только у статуса `conflict`.
// A6.4: «Копии…» — прежние версии (выбор штампа и «Вернуть»), «Показать в
// Проводнике…» — отдельное действие с подтверждением (В-A6-7).
function agResultActions(i){
  let html='<button onclick="agentResultOpen('+i+')">Открыть</button> '+
    '<button onclick="agentCompare('+i+')">Сравнить…</button> '+
    '<button onclick="agentBackupsForResult('+i+')">Копии…</button> '+
    '<button onclick="agentRevealRel(AG_RESULTS['+i+'] && AG_RESULTS['+i+'].rel)">Показать в Проводнике…</button> '+
    '<button onclick="agentDecide('+i+',\'accept\')">Принять</button> '+
    '<button onclick="agentDecide('+i+',\'reject\')">Отклонить</button>';
  if(AG_RESULTS[i] && AG_RESULTS[i].conflict){
    html+='<div class="muted" style="margin-top:4px">конфликт:</div>'+
      '<button onclick="agentDecide('+i+',\'conflict\',\'agent\')">Оставить вариант агента</button> '+
      '<button onclick="agentDecide('+i+',\'conflict\',\'new\')">Взять новую версию</button> '+
      '<button onclick="agentDecide('+i+',\'conflict\',\'delegate\')">Поручить агенту</button>';
  }
  return html;
}
async function loadAgentPending(){
  const tb=document.getElementById('agresults');
  if(!AG_ID){ tb.innerHTML=''; AG_RESULTS=[]; agResMsg(''); return; }
  agResMsg('…');
  try{
    const j=await jget('/api/agent/pending?project='+encodeURIComponent(AG_ID));
    if(j.error){ agResMsg(j.error, true); tb.innerHTML=''; AG_RESULTS=[]; return; }
    AG_RESULTS=j.items||[];
    agResMsg('ждут решения: '+AG_RESULTS.length+(j.conflicts? ' · есть копии в _conflicts':''));
    tb.innerHTML=AG_RESULTS.map(function(it,i){
      const status=(it.status==='conflict'? '<span class="err">конфликт</span>' : esc(it.status))+
        (it.variant? '' : ' <span class="muted">(варианта нет)</span>');
      return '<tr data-i="'+i+'"><td>'+esc(it.rel)+
        '<div class="muted">'+esc(it.anon_rel)+'</div></td>'+
        '<td>'+status+'</td><td>'+agReportLabel(it.report)+'</td>'+
        '<td>'+agResultActions(i)+'</td></tr>';
    }).join('')||
      '<tr><td colspan="4" class="muted">вариантов нет — правки агента появятся здесь после hds agent sync</td></tr>';
  }catch(e){ agResMsg('ошибка: '+e, true); }
}
// «Открыть» = показать путь и скопировать (В-A5-7: UI не запускает процессы).
function agentResultOpen(i){
  const it=AG_RESULTS[i]; if(!it) return;
  agentCopyPath(it.variant_path||'');
}
async function agentCompare(i){
  const it=AG_RESULTS[i]; if(!it) return;
  document.getElementById('agcompare').hidden=false;
  document.getElementById('agcmpTitle').textContent='Сравнение: '+it.rel;
  document.getElementById('agcmpMeta').textContent='…';
  document.getElementById('agcmpReport').textContent='';
  document.getElementById('agcmpDiff').textContent='';
  document.getElementById('agcmpmsg').textContent='';
  try{
    const j=await jget('/api/agent/compare?project='+encodeURIComponent(AG_ID)+
      '&rel='+encodeURIComponent(it.rel));
    if(j.error){
      document.getElementById('agcmpmsg').innerHTML='<span class="err">'+esc(j.error)+'</span>';
      document.getElementById('agcmpMeta').textContent='';
      return;
    }
    const line=function(name,f){
      if(!f || !f.exists) return '<div>'+esc(name)+': <span class="muted">нет файла</span></div>';
      return '<div>'+esc(name)+': '+esc(f.path)+' · '+fmtSize(f.size)+
        ' · sha256 '+(f.sha256? esc(f.sha256.slice(0,12)):'?')+'</div>';
    };
    document.getElementById('agcmpMeta').innerHTML=
      '<div>оригинал ↔ вариант'+(j.identical? ' <span class="muted">(файлы совпадают)</span>':'')+
      '</div>'+line('оригинал', j.original)+line('вариант', j.variant)+
      line('основание', j.base)+line('образ', j.image);
    const r=j.report;
    document.getElementById('agcmpReport').innerHTML = r
      ? ('<div>отчёт валидатора: '+
         (r.verdict==='check'? '<span class="err">проверить</span>':'<span class="ok">ок</span>')+
         ' · rev '+r.rev+'</div>'+(r.checks||[]).map(function(c){
           return '<div>· '+esc(c.name)+': '+
             (c.verdict==='check'? '<span class="err">проверить</span>':'ок')+
             ' — '+esc(c.detail)+'</div>'; }).join(''))
      : '<div>отчёта валидатора нет</div>';
    const diff=(j.diff||[]).join(NL);
    document.getElementById('agcmpDiff').textContent=
      diff || (j.diff_note? j.diff_note : 'различий в тексте нет');
    if(j.diff_truncated) document.getElementById('agcmpmsg').textContent='diff обрезан лимитом';
  }catch(e){
    document.getElementById('agcmpmsg').innerHTML='<span class="err">ошибка: '+esc(e)+'</span>';
  }
}
function agentCompareClose(){ document.getElementById('agcompare').hidden=true; }
async function agentDecide(i, decision, choice){
  const it=AG_RESULTS[i]; if(!it) return;
  if(decision==='accept' &&
     !confirm('Принять вариант? Оригинал будет заменён, прежняя версия — в .hds-agent\\backup.')) return;
  if(decision==='reject' &&
     !confirm('Отклонить правки агента? Образ вернётся к оригиналу, вариант будет удалён.')) return;
  if(decision==='conflict' && !confirm('Разрешить конфликт: '+choice+'?')) return;
  agResMsg('…');
  try{
    const body={project:AG_ID, rel:it.rel, decision:decision};
    if(choice) body.choice=choice;
    const j=await jpost('/api/agent/decision', body);
    if(j.error){ agResMsg(j.error, true); return; }
    let text='решение: '+(j.decision||decision)+(j.status? ' · статус '+j.status:'');
    if(j.backup) text+=' · backup: '+j.backup;
    if(j.conflict_copy) text+=' · копия для агента: '+j.conflict_copy;
    await loadAgentPending();
    await loadAgentFiles();
    await loadAgentState();
    agResMsg(text);
  }catch(e){ agResMsg('ошибка: '+e, true); }
}
// --- Копии, «Вернуть», «Пересоздать образ», relink (A6.4) ---
let AG_BACKUPS=[], AG_BKP_REL='';
function agBkpMsg(text, isErr){
  const m=document.getElementById('agbkpmsg');
  if(!m) return;
  if(isErr) m.innerHTML='<span class="err">'+esc(text)+'</span>'; else m.textContent=text||'';
}
function agBkpSize(n){
  if(n>=1048576) return (n/1048576).toFixed(1)+' МиБ';
  return Math.max(1, Math.round(n/1024))+' КиБ';
}
function agBkpTime(ts){
  try{ return new Date(ts*1000).toLocaleString(); }catch(e){ return ''; }
}
function agBkpRender(list){
  const sel=document.getElementById('agbkpStamp');
  sel.innerHTML=(list||[]).map(function(c){
    return '<option value="'+esc(c.stamp)+'">'+esc(c.stamp)+' · '+
      agBkpSize(c.size||0)+' · '+esc(agBkpTime(c.mtime))+'</option>';
  }).join('');
}
function agBkpSelected(){
  const sel=document.getElementById('agbkpStamp');
  const stamp=sel? sel.value : '';
  return AG_BACKUPS.find(function(c){ return c.stamp===stamp; })||null;
}
async function agentBackupsFor(rel){
  if(!AG_ID || !rel) return;
  AG_BKP_REL=rel; AG_BACKUPS=[];
  document.getElementById('agbackups').hidden=false;
  document.getElementById('agbkpTitle').textContent='Копии: '+rel;
  agBkpRender([]); agBkpMsg('…');
  try{
    const j=await jget('/api/agent/backups?project='+encodeURIComponent(AG_ID)+
      '&path='+encodeURIComponent(rel));
    if(j.error){ agBkpMsg(j.error, true); return; }
    AG_BACKUPS=j.copies||[];
    agBkpRender(AG_BACKUPS);
    agBkpMsg(AG_BACKUPS.length
      ? 'копий: '+AG_BACKUPS.length+' · выберите версию и нажмите «Вернуть»'
      : 'копий нет — вернуть нечего (копия делается при «Принять»)');
  }catch(e){ agBkpMsg('ошибка: '+e, true); }
}
function agentBackupsForResult(i){
  const it=AG_RESULTS[i]; if(it) agentBackupsFor(it.rel);
}
function agentBackupsForFile(i){
  const f=AG_FILES[i]; if(f) agentBackupsFor(f.orig);
}
function agentBackupsClose(){
  const box=document.getElementById('agbackups');
  if(box) box.hidden=true;
  AG_BACKUPS=[]; AG_BKP_REL='';
}
function agentCopySelectedCopy(){
  const copy=agBkpSelected();
  if(copy) agentCopyPath(copy.path||'');
  else agBkpMsg('выберите версию', true);
}
function agentRevealCopy(){
  const copy=agBkpSelected();
  if(!copy){ agBkpMsg('выберите версию', true); return; }
  agentRevealPath(copy.path, 'копия '+copy.stamp);
}
async function agentRestore(){
  if(!AG_BKP_REL){ return; }
  const copy=agBkpSelected();
  const stamp=copy? copy.stamp : '';
  if(!confirm('Вернуть «'+AG_BKP_REL+'» из копии '+(stamp||'(последняя)')+
     '? Текущая версия будет заменена, образ пересобран; копии останутся.')) return;
  agBkpMsg('…');
  try{
    const body={project:AG_ID, path:AG_BKP_REL};
    if(stamp) body.stamp=stamp;
    const j=await jpost('/api/agent/restore', body);
    if(j.error){ agBkpMsg(j.error, true); return; }
    const rel=AG_BKP_REL;
    await loadAgentPending(); await loadAgentFiles(); await loadAgentState();
    await agentBackupsFor(rel);
    agBkpMsg('вернули '+(j.stamp||'')+' · sha '+(j.sha256? j.sha256.slice(0,12):'?')+
      ' · статус '+j.status);
  }catch(e){ agBkpMsg('ошибка: '+e, true); }
}
async function agentRebuildRel(rel){
  if(!rel) return;
  if(!confirm('Пересоздать образ «'+rel+'» из текущего оригинала? Запись вернётся в статус synced.')) return;
  agResMsg('…');
  try{
    const j=await jpost('/api/agent/rebuild', {project:AG_ID, path:rel});
    if(j.error){ agResMsg(j.error, true); return; }
    await loadAgentPending(); await loadAgentFiles(); await loadAgentState();
    agResMsg('образ пересоздан · статус '+j.status+
      (j.sha256? ' · sha '+j.sha256.slice(0,12):''));
  }catch(e){ agResMsg('ошибка: '+e, true); }
}
function agentRebuildFile(i){
  const f=AG_FILES[i]; if(f) agentRebuildRel(f.orig);
}
// «Показать в Проводнике» — отдельное действие с явным согласием (В-A6-7):
// сервер запускает только explorer /select,<файл>, документы не открываются.
async function agentRevealPath(path, what){
  if(!path) return;
  if(!confirm('Показать '+(what||'файл')+' в Проводнике? Откроется Проводник с выделением '+
     'файла; приложения и документы не запускаются.')) return;
  try{
    const j=await jpost('/api/agent/reveal', {project:AG_ID, path:path});
    if(j.error){ agResMsg(j.error, true); return; }
    agResMsg('Проводник: '+j.path);
  }catch(e){ agResMsg('ошибка: '+e, true); }
}
function agentRevealRel(rel){ agentRevealPath(rel, rel); }
async function agentAdd(){
  const m=document.getElementById('agaddmsg'); m.textContent='…';
  const name=document.getElementById('agname').value.trim();
  const path=document.getElementById('agpath').value.trim();
  try{
    const j=await jpost('/api/agent/projects',{name:name,path:path});
    if(j.error){ m.innerHTML='<span class="err">'+esc(j.error)+'</span>'; return; }
    m.textContent='создан '+(j.project? j.project.id : '');
    document.getElementById('agname').value='';
    document.getElementById('agpath').value='';
    AG_ID='';
    await loadAgentProjects();
  }catch(e){ m.textContent='ошибка: '+e; }
}
// --- Удаление проекта (A6.5) ---
// Папка проекта не удаляется ни при каких флагах; живой демон зеркала —
// отказ: сервер вернёт 409 с подсказкой «hds stop», процессов UI не запускает.
async function agentRemoveProject(ev, id){
  if(ev && ev.stopPropagation) ev.stopPropagation();
  if(!id) return;
  if(!confirm('Удалить проект '+id+'? Уйдут каталог зоны (workspace, agent-config, '+
     'agent-state), реестр, запись в projects.json и токен с provider/model. '+
     'Папка проекта останется на месте.')) return;
  const m=document.getElementById('agmsg');
  if(m) m.textContent='удаление '+id+'…';
  try{
    const j=await jpost('/api/agent/project/rm',{project:id});
    if(j.error){
      if(m) m.innerHTML='<span class="err">'+esc(j.error)+'</span>';
      return;
    }
    AG_ID='';
    agentStreamClose(); agentBackupsClose(); agReset();
    if(m) m.textContent='проект '+id+' удалён';
    await loadAgentProjects();
  }catch(e){ if(m) m.innerHTML='<span class="err">ошибка: '+esc(String(e))+'</span>'; }
}
// --- Уведомления фильтра и «Очистить историю» (A5.6) ---
let AG_REDACT={total:0, labels:{}, sources:{}, last:[]};
function agRedactRender(){
  const box=document.getElementById('agredact');
  if(!box) return;
  box.hidden=AG_REDACT.total===0;
  const labels=Object.keys(AG_REDACT.labels).map(function(k){
    return k+' × '+AG_REDACT.labels[k]; }).join(', ');
  const srcs=Object.keys(AG_REDACT.sources).map(function(k){
    return k+' × '+AG_REDACT.sources[k]; }).join(', ');
  document.getElementById('agredactsum').textContent=AG_REDACT.total?
    ('срабатываний: '+AG_REDACT.total+(labels? ' · метки: '+labels:'')+
     (srcs? ' · источник: '+srcs:'')):'';
  document.getElementById('agredactlast').textContent=AG_REDACT.last.length?
    ('последнее: '+AG_REDACT.last.join(', ')):'';
}
function agentRedactReset(){
  AG_REDACT={total:0, labels:{}, sources:{}, last:[]};
  agRedactRender();
}
function agHistMsg(text, isErr){
  const a=document.getElementById('aghistmsg'), b=document.getElementById('agredactmsg');
  [a, b].forEach(function(m){
    if(!m) return;
    if(isErr) m.innerHTML='<span class="err">'+esc(text)+'</span>'; else m.textContent=text;
  });
}
// «Очистить историю» (TR-6.6): стоп → метки вместо значений → отчёт. Своих
// разборов в JS нет — сервер отдаёт {session, files, backups, replaced}.
async function agentClearHistory(){
  if(!AG_ID){ agHistMsg('сначала выберите проект', true); return; }
  if(!confirm('Очистить историю агента?'+NL+NL+
      'Будет: остановить сессию, заменить значения в файлах истории метками '+NL+
      '(рядом останутся бэкапы) и возобновить сессию следующим ходом.'+NL+NL+
      'Продолжить?')) return;
  agHistMsg('…');
  try{
    const j=await jpost('/api/agent/history/clear', {project:AG_ID});
    if(j.error){ agHistMsg(j.error, true); return; }
    const files=(j.files||[]).length||j.files_count||0;
    const backups=(j.backups||[]).length||j.backups_count||0;
    agHistMsg('история очищена: сессия '+(j.session||'-')+' · файлов '+files+
      ' · бэкапов '+backups+' · замен '+(j.replaced||0));
    agentRedactReset();
    loadAgentState(); loadAgentSettings();
  }catch(e){ agHistMsg('ошибка: '+e, true); }
}
// --- Настройки агента (A5.6): таблица «параметр → значение → изменить» ---
let AG_SET=null, AG_SETROWS=[];
function agSetMsg(text, isErr){
  const m=document.getElementById('agsetmsg');
  if(!m) return;
  if(isErr) m.innerHTML='<span class="err">'+esc(text)+'</span>'; else m.textContent=text;
}
function agBool(v){ return v? '<span class="ok">да</span>':'<span class="muted">нет</span>'; }
// Строки таблицы строятся из ответа сервера: своих значений и регулярок в JS нет.
function agBuildRows(j){
  const p=j.project||{}, c=j.config||{};
  const rows=[
    {label:'Провайдер', field:'provider', kind:'text', raw:p.provider||'', view:esc(p.provider||'-')},
    {label:'Модель', field:'model', kind:'text', raw:p.model||'', view:esc(p.model||'-')},
    {label:'Ключ провайдера', field:'api_key', kind:'secret',
      raw:(p.key&&p.key.set)?'set':'', view:(p.key&&p.key.set)?
        '<span class="ok">задан</span>':'<span class="err">не задан</span>'},
    {label:'База API', field:'base_url', kind:'text', raw:p.base_url||'',
      view:esc(p.base_url||'(по умолчанию)')},
    {label:'Зона включена', field:'enabled', kind:'bool', raw:c.enabled, view:agBool(c.enabled)},
    {label:'Корень зоны', field:'root', kind:'text', raw:c.root||'', view:esc(c.root||'-')},
    {label:'Неизвестные значения', field:'unknown', kind:'select', raw:c.unknown||'label',
      view:esc(c.unknown||'label')},
    {label:'Снимок vault, сек', field:'filter_max_stale_sec', kind:'num',
      raw:c.filter_max_stale_sec, view:esc(String(c.filter_max_stale_sec||''))},
    {label:'Журнал исходящего', field:'egress_log', kind:'bool', raw:c.egress_log,
      view:agBool(c.egress_log)},
    {label:'Предпросмотр в CLI', field:'preview_outgoing', kind:'bool', raw:c.preview_outgoing,
      view:agBool(c.preview_outgoing)},
    {label:'Суб-агенты', field:'subagents', kind:'bool', raw:c.subagents, view:agBool(c.subagents)},
    {label:'Живых проектов', field:'max_live_projects', kind:'num', raw:c.max_live_projects,
      view:esc(String(c.max_live_projects||''))},
    {label:'Таймаут команды, сек', field:'request_timeout_sec', kind:'num',
      raw:c.request_timeout_sec, view:esc(String(c.request_timeout_sec||''))},
    {label:'Таймаут вопроса, сек', field:'ask_timeout_sec', kind:'num',
      raw:c.ask_timeout_sec, view:esc(String(c.ask_timeout_sec||''))},
    {label:'Прокси', field:'proxy', kind:'text', raw:c.proxy||'',
      view:esc(c.proxy||'(нет)')},
    {label:'MCP disk-search', field:'disk_search_enabled', kind:'bool',
      raw:c.disk_search_enabled, view:agBool(c.disk_search_enabled)},
    {label:'MCP 1c-buddy', field:'onec_enabled', kind:'bool', raw:c.onec_enabled,
      view:agBool(c.onec_enabled)},
    {label:'Команда 1c-buddy', field:'onec_command', kind:'text', raw:c.onec_command||'',
      view:esc(c.onec_command||'(из secrets/node_modules)')},
    {label:'MCP tavily', field:'tavily_enabled', kind:'bool', raw:c.tavily_enabled,
      view:agBool(c.tavily_enabled)},
    {label:'Команда tavily', field:'tavily_command', kind:'text', raw:c.tavily_command||'',
      view:esc(c.tavily_command||'(node_modules воркера)')}
  ];
  const live=(j.live||[]).map(function(l){ return l.project+' ('+l.idle_sec+' с)'; }).join(', ');
  rows.push({label:'Живые процессы', field:'', kind:'info', raw:live, view:esc(live||'нет')});
  rows.push({label:'Файлы', field:'', kind:'info', raw:'',
    view:'<span class="muted">config: '+esc(j.config_path||'?')+'<br>secrets: '+
      esc(j.secrets_path||'?')+'</span>'});
  return rows;
}
function agSettingsTable(){
  const tb=document.getElementById('agsettings');
  if(!tb) return;
  tb.innerHTML=AG_SETROWS.map(function(r, i){
    return '<tr><td>'+esc(r.label)+'</td><td>'+r.view+'</td><td>'+
      (r.field? '<button onclick="agentSetEdit('+i+')">Изменить</button>'
        : '<span class="muted">—</span>')+'</td></tr>';
  }).join('');
}
async function loadAgentSettings(){
  if(!AG_ID){
    AG_SET=null; AG_SETROWS=[];
    const tb=document.getElementById('agsettings'); if(tb) tb.innerHTML='';
    agSetMsg(''); return;
  }
  agSetMsg('…');
  try{
    const j=await jget('/api/agent/settings?project='+encodeURIComponent(AG_ID));
    if(j.error){ agSetMsg(j.error, true); AG_SET=null; AG_SETROWS=[]; agSettingsTable(); return; }
    AG_SET=j; AG_SETROWS=agBuildRows(j);
    agSettingsTable();
    agSetMsg('ключ: '+((j.project&&j.project.key&&j.project.key.set)? 'задан':'не задан')+
      ' · живых процессов: '+((j.live||[]).length));
  }catch(e){ agSetMsg('ошибка: '+e, true); }
}
async function agentSetSave(body){
  agSetMsg('…');
  try{
    const j=await jpost('/api/agent/settings', body);
    if(j.error){ agSetMsg(j.error, true); return; }
    AG_SET=j; AG_SETROWS=agBuildRows(j); agSettingsTable();
    let text='сохранено: '+((j.applied||[]).join(', ')||'без изменений')+
      (j.reopened? ' · сессия откроется заново':'')+
      (j.unloaded? ' · процесс выгружен':'');
    if((j.warnings||[]).length) text+=' · '+j.warnings.join('; ');
    agSetMsg(text);
    loadAgentState(); loadAgentProjects(); loadAgentFiles();
  }catch(e){ agSetMsg('ошибка: '+e, true); }
}
function agentSetSelect(i){
  const r=AG_SETROWS[i], el=document.getElementById('agsetselect');
  if(!r||!el) return;
  const body={project:AG_ID};
  body[r.field]=el.value;
  agentSetSave(body);
}
// Правка одной строки: prompt() для скаляров, select для `unknown`, confirm()
// для bool и для того, что перезапускает сессию (В-A5.6-3).
async function agentSetEdit(i){
  const r=AG_SETROWS[i];
  if(!r||!r.field) return;
  if(r.kind==='bool'){
    if(!confirm(r.label+': сейчас '+(r.raw? 'включено':'выключено')+'. Переключить?')) return;
    const body={project:AG_ID};
    body[r.field]=!r.raw;
    agentSetSave(body); return;
  }
  if(r.kind==='select'){
    const tb=document.getElementById('agsettings');
    const cell=tb&&tb.rows[i]? tb.rows[i].cells[1]:null;
    if(!cell) return;
    cell.innerHTML='<select id="agsetselect"><option value="label">label</option>'+
      '<option value="block">block</option></select> '+
      '<button onclick="agentSetSelect('+i+')">Сохранить</button> '+
      '<button onclick="loadAgentSettings()">Отмена</button>';
    document.getElementById('agsetselect').value=r.raw||'label';
    return;
  }
  const current=(r.kind==='secret')? '':String(r.raw===null||r.raw===undefined? '':r.raw);
  const input=window.prompt(r.label+' — '+
    (r.kind==='secret'? 'новый ключ (значение не показывается)':'новое значение'), current);
  if(input===null) return;
  const value=input.trim();
  if(r.kind==='secret' && !value){ agSetMsg('ключ пустой — не меняем', true); return; }
  const body={project:AG_ID};
  if(r.kind==='num'){
    const n=Number(value);
    if(!isFinite(n) || Math.floor(n)!==n){ agSetMsg('нужно целое число', true); return; }
    body[r.field]=n;
  }else{
    body[r.field]=value;
  }
  if(r.kind==='secret'||r.field==='provider'||r.field==='model'||r.field==='root'){
    const warn=(r.field==='root'?
      'Корень зоны меняется: сессия будет выгружена, зона станет другой. Продолжить?':
      'Смена применится следующим ходом — сессия откроется заново. Продолжить?');
    if(!confirm(warn)) return;
  }
  agentSetSave(body);
}
// --- Транскрибация (PLAN_AUTO_TRANSCRIBE §8) ---
function showTab(name){
  const tabs=['search','transcribe','anonymize','agent'];
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
  if(cur==='agent'){ loadAgentProjects(); if(AG_ID) agentStreamOpen(); }
  else { agentStreamClose(); }
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
  const known=['search','transcribe','anonymize','agent'];
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A6.5: кнопка удаления не должна потеряться в панели проектов.
    #[test]
    fn page_has_project_remove_button() {
        assert!(
            PAGE.contains("agentRemoveProject"),
            "в панели проектов нет кнопки «Удалить…»"
        );
        assert!(
            PAGE.contains("/api/agent/project/rm"),
            "кнопка не вызывает маршрут удаления"
        );
        assert!(
            PAGE.contains("stopPropagation"),
            "кнопка должна гасить клик по строке (selectAgentProject)"
        );
        assert!(
            PAGE.contains("не очищена"),
            "панель состояния должна показывать предупреждение AG-32"
        );
    }
}
