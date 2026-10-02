# W4_REPORT.md — журнал волны W4 (упаковка/установка/CI)

> Ветка `w2-llm-host` (W4 продолжаем в ней), начато **01.10.2026**. План —
> `MIGRATION_PLAN_RUST.md` §10 (артефакты/установка) и §11 (CI).

## 1. CI для Rust + релизная сборка (01.10.2026)

**Что сделано.**

| Файл | Что внутри |
|---|---|
| `.github/workflows/rust.yml` | `test-rust` (ubuntu: `cargo clippy --workspace` информативно + `cargo test --workspace`), `build-windows` (windows: `cargo build --release -p hds-cli -p hds-mcp -p hds-llama` + upload `hds.exe`/`hds_mcp.exe`/`llm_host.exe`) |
| `installers/build_rust_release.ps1` | релизная сборка + стейджинг `dist\hds-<ver>-windows-x64\` (bin с 3 exe, `config.example.yaml`, `README.md`, `sha256.txt`); ASCII-only |
| `.gitignore` | `dist/` |

**Решения.**
* `clippy` — **информативно**, без `-D warnings`: в воркспейсе 61 предупреждение,
  `cargo fmt` не применён. Очистка предупреждений и форматирование — отдельная
  задача **W5 (очистка)**; блокирующие в CI — только тесты и сборка.
* Python-регрессия остаётся в `ci.yml` (пока Python есть), новая матрица — `rust.yml`.
* Windows-артефакт — один вариант (без whisper-сборки): whisper даёт рантайм движка.

**Живой прогон.**
```
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0
# [w4] staged: dist\hds-0.1.0-windows-x64  (bin\hds.exe, bin\hds_mcp.exe, bin\llm_host.exe,
#                                           config.example.yaml, README.md, sha256.txt)
dist\hds-0.1.0-windows-x64\bin\hds.exe --help   # ok
```

**Грабли (уже видны).** Релизная сборка **требует остановленного резидента**:
живой `target\release\llm_host.exe` (владелец портов) держит свой exe → `cargo build`
падает `os error 5`. Текущая процедура: `llm-host stop` → сборка → запуск. Долгосрочно
(§10.6) — версионные каталоги `app\<ver>\` + указатель `app\current` и перезапуск
задачи; это отдельная задача W4.

**Дальше по W4:** `setup.ps1`/`install_windows.ps1` под Rust-бинарники (`bin\hds.exe`,
автозапуск `llm-host` вместо Python-ролей), доставка ONNX-моделей CLIP и рантайма ASR
(`engine-manifest.json` + sha256, джоба `fetch-engine-runtime`), `package` (zip + sha256),
`release` с бинарными ассетами.
