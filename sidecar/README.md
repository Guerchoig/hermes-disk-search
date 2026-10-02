# sidecar — Python-воркер извлечения и нормализации (B6)

Автономный процесс для того, что по плану остаётся в Python
(`MIGRATION_PLAN_RUST.md` §2.5, §5): извлечение сегментов (PDF/DOCX/XLSX/PPTX/OCR)
и лемматизация FTS (`pymorphy3`). Rust-ядро говорит с ним по **stdio + JSON-RPC 2.0**
(NDJSON), запускает лениво и владеет процессом.

## Состав

| Файл | Что |
|---|---|
| `hds_extract/worker.py` | воркер: `hello`/`extract`/`normalize`/`clip_image`/`shutdown`, idle-timeout, изоляция stdout |
| `hds_extract/requirements.lock` | зависимости воркера (вариант A, сняты со спайка 3) |

## Запуск (разработка)

```powershell
# воркер использует зависимости окружения проекта (.venv)
'{"jsonrpc":"2.0","id":1,"method":"hello"}' |
  .\.venv\Scripts\python.exe sidecar\hds_extract\worker.py --root .
```

Rust-клиент (`crates/hds-extract`) ищет интерпретатор так:
`HDS_EXTRACT_PYTHON` → `sidecar/python/**/python.exe` (вариант A, установщик) →
`.venv\Scripts\python.exe` (вариант C, fallback) → ошибка с подсказкой.

## Поставка (вариант A, самодостаточный)

`installers/build_sidecar.ps1` собирает дерево, работающее **без** Python-ядра проекта:
`hds_extract/` (воркер + lock), `python/` (портативный CPython из python-build-standalone +
зависимости из `requirements.lock`), `hds/` (**копия** модулей извлечения/лемматизации из
корневого `hds/*.py` — источник истины не переезжает), `README.md`. `worker.py` ставит
каталог `sidecar/` в `sys.path` перед корнем проекта: в поставке `hds` резолвится из копии,
в разработке (`sidecar/hds` отсутствует) — из корня проекта.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

В релизе это делает джоба CI `build-sidecar`; `build-windows` кладёт готовое дерево в пакет.

## Контракт (§5 плана)

* транспорт — stdio, NDJSON, UTF-8; аргумент `--root <проект>` (воркер не ищет `config.yaml` сам);
* `hello` → `{protocol: 1, python, pid, capabilities}`; родитель сверяет `protocol` и возможности;
* `extract {path, opts}` → `{kind, segments[], warnings[], elapsed_ms}` либо
  структурированная ошибка `{code, message, hint}`; сегменты — как `hds/extractors.seg()`
  (`text/page/t_start/t_end`, опционально `head`);
* `normalize {texts[]}` → `{lemmas[]}`;
* `clip_image` → воркер отвечает «не поддерживаю» (CLIP переезжает в Rust на ONNX, §7);
* `shutdown` либо закрытие stdin → штатный выход;
* простой дольше `extract.idle_timeout` (по умолчанию 60 с) → воркер завершается сам.

## Грабли (W0/спайк 3)

* библиотеки и `ffmpeg` пишут в **fd 1** напрямую — воркер дублирует `fd 1`, уводит
  `fd 1` в stderr, протокол пишет в дубликат;
* Python на Windows читает stdin в ANSI — принудительно UTF-8 (кириллица в путях);
* `select()` по pipe на Windows не работает — читатель в отдельном потоке;
* `uv venv` python.exe — процесс-трамплин: память мерить по дереву процессов
  (`tools/parity/proc_tree.ps1`).