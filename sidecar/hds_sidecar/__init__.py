# Общий пакет sidecar-воркеров (PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §5.1).
#
# Пусто намеренно: импорты на уровне пакета потянули бы чужие зависимости
# (torch, fitz, pymorphy3) в процесс, которому они не нужны. Воркеры импортируют
# конкретные модули: hds_sidecar.stdio / .runtime / .config / .progress / .fs.
