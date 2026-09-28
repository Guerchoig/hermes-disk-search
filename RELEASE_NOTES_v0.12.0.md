# RELEASE_NOTES v0.12.0 — автонастройка OCR (Tesseract) в инсталляторах

## Новое

### OCR настраивается автоматически при установке (Windows и macOS)

Раньше после установки OCR часто не работал из-за двух подводных камней:

1. hermes-disk-search стартует из ярлыка, LaunchAgent или MCP-сервера — в таком
   окружении **PATH минимален**, и `tesseract.exe` из `C:\Program Files\Tesseract-OCR`
   (Windows) или `/opt/homebrew/bin` (macOS) не находился: в UI появлялось
   «Tesseract OCR не найден», хотя Tesseract был установлен;
2. при `ocr_lang="rus+eng"` базовая поставка UB-Mannheim (Windows) содержит
   **только английский** языковой пакет — распознавание русского падало.

Теперь оба инсталлятора (`setup.ps1` и `installers/install_macos.command`)
запускают новый шаг **`installers/configure_ocr.py`**, который:

- ищет tesseract: PATH → стандартные каталоги установки (Program Files,
  `%LOCALAPPDATA%\Programs`, Homebrew);
- обеспечивает языки **rus+eng**: отсутствующие пакеты `*.traineddata` скачиваются
  в пользовательский каталог `%LOCALAPPDATA%\Tesseract-OCR\tessdata`
  (Windows) — **без прав администратора**; HDS сам подключает этот каталог через
  `TESSDATA_PREFIX`. На macOS языки ставятся через `brew install tesseract-lang`;
- прописывает найденный путь в новый ключ конфига **`index.ocr_tesseract_cmd`**
  (config.yaml создаётся при отсутствии, существующие настройки и комментарии
  сохраняются).

Отсутствие Tesseract или интернета **не является ошибкой установки** — печатается
подсказка (`winget install -e --id UB-Mannheim.TesseractOCR` /
`brew install tesseract tesseract-lang`), установку это не блокирует.

Повторный запуск вручную в любой момент:

```powershell
# Windows
.venv\Scripts\python.exe installers\configure_ocr.py
```
```bash
# macOS
.venv/bin/python installers/configure_ocr.py
```

## Исправления

- Отдельных исправлений ошибок в этом релизе нет — правка ограничена
  инфраструктурой установки: `config.example.yaml` и дефолтный конфиг дополнены
  ключом `index.ocr_tesseract_cmd` (пустое значение = прежнее поведение).

## Требования к обновлению

- Переиндексация **не** требуется (схема БД не менялась).
- Ключ `index.ocr_tesseract_cmd` необязателен: при его отсутствии поведение
  прежнее. Если OCR в UI помечен как «не найден» — обновите установку
  (`setup.ps1` / `installers/install_macos.command`) или запустите
  `installers/configure_ocr.py` вручную.
- Общий MCP-сервер перезапустите при необходимости:
  `python -m hds.cli mcp-http restart-if-stale` (инсталляторы делают это сами).

## Проверки

- 269 регрессионных тестов (`python -m unittest discover -s tests`) — зелёные
  (2 пропущены, как и раньше);
- `hds.cli check` на реальной машине — без критических проблем
  (Tesseract найден, чат/эмбеддинги/реранкер отвечают);
- ручная приёмка сценария «спроси в чате Hermes → ответ со ссылками» —
  MCP-инструменты disk-search возвращают фрагменты с путями и страницами.
