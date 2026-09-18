# Диагностика медленной индексации на второй машине (AMD / Vulkan)

Коротко: в проекте Vulkan (whisper.cpp) влияет **только на транскрипцию
аудио/видео**. Офисные файлы (Word/Excel/PDF) проходят путь
`извлечение текста (CPU) → чанки → эмбеддинги через LM Studio → index.db`,
и единственное GPU-звено в нём — эмбеддинги в LM Studio.

Замеры на эталонной машине (RTX 3060) показали:
- эмбеддинги: CUDA 33 мс/чанк, Vulkan-рантайм 35 мс/чанк — **паритет**;
- транскрипция (2 мин аудио, модель small): прод-CUDA (батч+VAD) RTF 0.010,
  CUDA без батчей RTF 0.135, Vulkan/whisper.cpp на AMD iGPU RTF 0.119,
  та же сборка whisper.cpp на RTX 3060 RTF 0.039.

То есть «Vulkan медленный» — в основном **режим** (нет батчей/VAD у whisper.cpp),
а не бэкенд. Для офисных файлов ищите проблему в LM Studio, OCR или диске.

## Шаги на диагностируемой машине

1. Убедиться, что LM Studio запущен и сервер включён
   (Developer → Start Server), модель эмбеддингов (`text-embedding-bge-m3`)
   скачана.

2. Проверить, что рядом не висит большая чат-модель:

       lms ps

   Если видна qwen3.5-9b и т.п. — выгрузить на время индексации:
   `lms unload <идентификатор>`.

3. Запустить диагностику из корня проекта:

       .venv\Scripts\python.exe tools\diag_index_speed.py

   Скрипт проверит сервер, покажет загруженные модели, померяет скорость
   эмбеддингов ровно тем же кодом, что и индексатор (`Embedder.embed`,
   `embedding.batch_size`), и напечатает вердикт:
   - **< 80 мс/чанк** — модель на GPU, эмбеддинги не при чём;
   - **80–250 мс/чанк** — частичный офлоад, в LM Studio для bge-m3
     выставить GPU-offload = max;
   - **> 250 мс/чанк** — модель считается на CPU. Это и есть главная
     вероятная причина медленной индексации офисных файлов. Лечится
     загрузкой bge-m3 с полным офлоадом в LM Studio (проверить рантайм:
     `lms runtime ls`, при необходимости `lms runtime select`).

4. Если хочется сравнить и транскрипцию — положить тестовый wav и указать его:

       .venv\Scripts\python.exe tools\diag_index_speed.py --audio путь\к\аудио.wav

   Эталон: RTF 0.04 (RTX 3060) / 0.12 (AMD iGPU) на модели small.
   RTF > 0.3 — смотрите `tools/bench_paths.py transcribe vulkan <файл>`.

5. Офисные файлы всё ещё медленные при быстрой п.3? Тогда:
   - OCR: в `config.yaml` `index.ocr: true` — Tesseract работает на CPU.
     Сравните прогон с `ocr: false`;
   - диск: если корни на HDD, тормозят чтение и `content_hash`;
   - антивирус: реалтайм-сканирование каждого файла и папки `.venv`.

## Дополнительно: полный бенчмарк (как на эталонной машине)

    .venv\Scripts\python.exe tools\bench_paths.py embed --batches 4
    .venv\Scripts\python.exe tools\bench_paths.py transcribe vulkan test_data\speech_2min.wav
    .venv\Scripts\python.exe tools\bench_paths.py transcribe cuda   test_data\speech_2min.wav
