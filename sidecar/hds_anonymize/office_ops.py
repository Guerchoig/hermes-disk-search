"""Операции с документами Office для агента (TR-3.5).

Встроенный `editor` SDK правит только текстовые файлы (`md`, `txt`, `csv`,
`json`), поэтому `docx`/`xlsx`/`pptx` обслуживает Python-воркер через RPC
(в A3 — через `host_call`):

* `office_read(path)` — текстовое представление: абзацы с номерами, таблицы
  в markdown, листы и ячейки с адресами, слайды;
* `office_edit(path, ops[])` — `replace_text`, `set_cell`, `add_row`,
  `add_column`, `insert_paragraph`, `delete_paragraph`; числа, даты и
  формулы XLSX не пересоздаются, стили абзацев DOCX сохраняются;
* `office_create(path, kind, content)` — новый `docx` из markdown, `xlsx` из
  таблиц, `pptx` из слайдов «заголовок + тезисы».

Модуль работает на путях **внутри анонимной зоны**: область действия
ограничивает хук `hds-guard` (A3), здесь проверяется только формат и
существование родительского каталога.
"""
from __future__ import annotations

import logging
import re
from pathlib import Path
from typing import Iterable, Optional

logger = logging.getLogger("hds_anonymize.office")


class OfficeError(ValueError):
    """Операция над документом Office невозможна (формат/параметры)."""


SUPPORTED = {".docx", ".xlsx", ".pptx"}
_DOCX = ".docx"
_XLSX = ".xlsx"
_PPTX = ".pptx"


def _require(path: str, ext: str) -> Path:
    p = Path(path)
    if p.suffix.lower() != ext:
        raise OfficeError(f"ожидался файл {ext}, получен {p.suffix or p.name}")
    if not p.is_file():
        raise FileNotFoundError(f"Файл не найден: {p}")
    return p


def _rows_to_markdown(rows: Iterable[Iterable[object]]) -> str:
    rows = [["" if cell is None else str(cell) for cell in row] for row in rows]
    if not rows:
        return ""
    width = max(len(row) for row in rows)
    rows = [row + [""] * (width - len(row)) for row in rows]
    header, *rest = rows
    lines = ["| " + " | ".join(header) + " |",
             "| " + " | ".join("---" for _ in header) + " |"]
    for row in rest:
        lines.append("| " + " | ".join(row) + " |")
    return "\n".join(lines)


def _set_paragraph_text(paragraph, text: str) -> None:
    """Заменить текст абзаца, сохранив стиль абзаца и первого run."""
    runs = paragraph.runs
    if runs:
        runs[0].text = text
        for run in runs[1:]:
            run.text = ""
    else:
        paragraph.add_run(text)


# ── чтение ───────────────────────────────────────────────────────────────


def office_read(path: str) -> dict:
    """Текстовое представление документа (TR-3.5)."""
    p = Path(path)
    ext = p.suffix.lower()
    if ext == _DOCX:
        return _read_docx(p)
    if ext == _XLSX:
        return _read_xlsx(p)
    if ext == _PPTX:
        return _read_pptx(p)
    raise OfficeError(f"office_read: неподдерживаемый формат {ext or p.name}")


def _read_docx(path: Path) -> dict:
    import docx  # noqa: PLC0415

    doc = docx.Document(str(path))
    paragraphs = [
        {"index": index, "text": para.text, "style": getattr(para.style, "name", "")}
        for index, para in enumerate(doc.paragraphs, start=1)
    ]
    tables = [
        [[cell.text for cell in row.cells] for row in table.rows]
        for table in doc.tables
    ]
    lines = [f"[{item['index']}] {item['text']}" for item in paragraphs]
    for number, rows in enumerate(tables, start=1):
        lines.extend(["", f"Таблица {number}:", _rows_to_markdown(rows)])
    return {
        "path": str(path),
        "kind": "docx",
        "paragraphs": paragraphs,
        "tables": tables,
        "markdown": "\n".join(lines),
    }


def _read_xlsx(path: Path) -> dict:
    import openpyxl  # noqa: PLC0415

    wb = openpyxl.load_workbook(str(path), data_only=False)
    sheets = []
    try:
        for ws in wb.worksheets:
            cells = []
            for row in ws.iter_rows():
                for cell in row:
                    if cell.value is None:
                        continue
                    cells.append({
                        "address": cell.coordinate,
                        "value": cell.value,
                        "number_format": cell.number_format,
                    })
            sheets.append({
                "name": ws.title,
                "max_row": ws.max_row,
                "max_column": ws.max_column,
                "cells": cells,
                "markdown": _rows_to_markdown(
                    [["" if cell.value is None else cell.value for cell in row]
                     for row in ws.iter_rows()],
                ),
            })
    finally:
        wb.close()
    return {
        "path": str(path),
        "kind": "xlsx",
        "sheets": sheets,
        "markdown": "\n\n".join(
            f"## {sheet['name']}\n{sheet['markdown']}" for sheet in sheets
        ),
    }


def _read_pptx(path: Path) -> dict:
    try:
        from pptx import Presentation  # noqa: PLC0415
    except ImportError as exc:  # pragma: no cover — зависит от venv
        raise OfficeError(
            "python-pptx не установлен в venv анонимизации "
            "(installers/build_anonymize_sidecar.ps1)"
        ) from exc
    prs = Presentation(str(path))
    slides = []
    for index, slide in enumerate(prs.slides, start=1):
        title = ""
        bullets: list[str] = []
        for shape in slide.shapes:
            if not getattr(shape, "has_text_frame", False):
                continue
            text = shape.text_frame.text.strip()
            if not text:
                continue
            if not title and getattr(shape, "is_placeholder", False):
                title = text.splitlines()[0]
                bullets.extend(text.splitlines()[1:])
            else:
                bullets.extend(text.splitlines())
        slides.append({"index": index, "title": title, "bullets": bullets})
    return {
        "path": str(path),
        "kind": "pptx",
        "slides": slides,
        "markdown": "\n\n".join(
            f"## Слайд {slide['index']}"
            + (f"\n# {slide['title']}" if slide["title"] else "")
            + "".join(f"\n- {item}" for item in slide["bullets"])
            for slide in slides
        ),
    }


# ── правка ───────────────────────────────────────────────────────────────


def office_edit(path: str, ops: list[dict]) -> dict:
    """Применить операции к документу (TR-3.5)."""
    p = Path(path)
    ext = p.suffix.lower()
    if not ops:
        raise OfficeError("office_edit: пустой список операций")
    if ext == _DOCX:
        return _edit_docx(p, ops)
    if ext == _XLSX:
        return _edit_xlsx(p, ops)
    if ext == _PPTX:
        return _edit_pptx(p, ops)
    raise OfficeError(f"office_edit: неподдерживаемый формат {ext or p.name}")


def _docx_paragraphs(doc):
    """Все абзацы документа: тело + ячейки таблиц верхнего уровня."""
    for para in doc.paragraphs:
        yield para
    for table in doc.tables:
        for row in table.rows:
            for cell in row.cells:
                for para in cell.paragraphs:
                    yield para


def _edit_docx(path: Path, ops: list[dict]) -> dict:
    import docx  # noqa: PLC0415

    doc = docx.Document(str(path))
    applied = []
    warnings: list[str] = []
    for number, op in enumerate(ops, start=1):
        kind = (op.get("op") or op.get("kind") or "").strip()
        if kind == "replace_text":
            find = str(op.get("find") or "")
            replace = str(op.get("replace") or "")
            if not find:
                raise OfficeError(f"op #{number}: replace_text без find")
            use_regex = bool(op.get("regex"))
            pattern = re.compile(find if use_regex else re.escape(find))
            count = 0
            for para in _docx_paragraphs(doc):
                if not pattern.search(para.text):
                    continue
                new_text = pattern.sub(replace, para.text)
                _set_paragraph_text(para, new_text)
                count += 1
            applied.append({"op": kind, "paragraphs": count})
        elif kind == "set_cell":
            table = doc.tables[int(op["table"])]
            cell = table.rows[int(op["row"])].cells[int(op["col"])]
            _set_paragraph_text(cell.paragraphs[0], str(op.get("text") or ""))
            applied.append({"op": kind})
        elif kind == "add_row":
            table = doc.tables[int(op["table"])]
            values = [str(item) for item in (op.get("values") or [])]
            row = table.add_row()
            for index, value in enumerate(values):
                if index < len(row.cells):
                    _set_paragraph_text(row.cells[index].paragraphs[0], value)
            applied.append({"op": kind, "values": len(values)})
        elif kind == "add_column":
            table = doc.tables[int(op["table"])]
            header = str(op.get("header") or "")
            table.add_column(0)  # ширина в Emu; 0 — автоподбор
            if header:
                last_row = table.rows[0]
                _set_paragraph_text(last_row.cells[-1].paragraphs[0], header)
            applied.append({"op": kind})
        elif kind == "insert_paragraph":
            index = int(op.get("index") or len(doc.paragraphs))
            text = str(op.get("text") or "")
            style: Optional[str] = op.get("style") or None
            para = doc.add_paragraph(text, style=style)
            if doc.paragraphs and index <= len(doc.paragraphs) - 1:
                doc.paragraphs[index - 1]._p.addprevious(para._p)
            applied.append({"op": kind, "index": index})
        elif kind == "delete_paragraph":
            index = int(op["index"])
            element = doc.paragraphs[index - 1]._p
            element.getparent().remove(element)
            applied.append({"op": kind, "index": index})
        else:
            raise OfficeError(f"op #{number}: неизвестная операция {kind!r}")
    doc.save(str(path))
    return {"path": str(path), "kind": "docx", "applied": applied,
            "warnings": warnings}


def _edit_xlsx(path: Path, ops: list[dict]) -> dict:
    import openpyxl  # noqa: PLC0415

    wb = openpyxl.load_workbook(str(path), data_only=False)
    applied = []
    try:
        for number, op in enumerate(ops, start=1):
            kind = (op.get("op") or op.get("kind") or "").strip()
            if kind == "set_cell":
                ws = wb[op["sheet"]] if op.get("sheet") else wb.active
                cell = ws[str(op["cell"])]
                cell.value = op.get("value")  # формула остаётся формулой
                applied.append({"op": kind, "cell": cell.coordinate})
            elif kind == "add_row":
                ws = wb[op["sheet"]] if op.get("sheet") else wb.active
                ws.append(list(op.get("values") or []))
                applied.append({"op": kind, "row": ws.max_row})
            elif kind == "add_column":
                ws = wb[op["sheet"]] if op.get("sheet") else wb.active
                column = ws.max_column + 1
                for row_index, value in enumerate(
                    list(op.get("values") or []), start=1
                ):
                    ws.cell(row=row_index, column=column, value=value)
                applied.append({"op": kind, "column": column})
            elif kind == "replace_text":
                find = str(op.get("find") or "")
                replace = str(op.get("replace") or "")
                if not find:
                    raise OfficeError(f"op #{number}: replace_text без find")
                count = 0
                sheets = [wb[op["sheet"]]] if op.get("sheet") else wb.worksheets
                for ws in sheets:
                    for row in ws.iter_rows():
                        for cell in row:
                            if isinstance(cell.value, str) and find in cell.value:
                                cell.value = cell.value.replace(find, replace)
                                count += 1
                applied.append({"op": kind, "cells": count})
            else:
                raise OfficeError(f"op #{number}: неизвестная операция {kind!r}")
        wb.save(str(path))
    finally:
        wb.close()
    return {"path": str(path), "kind": "xlsx", "applied": applied,
            "warnings": []}


def _edit_pptx(path: Path, ops: list[dict]) -> dict:
    try:
        from pptx import Presentation  # noqa: PLC0415
    except ImportError as exc:  # pragma: no cover
        raise OfficeError("python-pptx не установлен в venv анонимизации") from exc
    prs = Presentation(str(path))
    applied = []
    for number, op in enumerate(ops, start=1):
        kind = (op.get("op") or op.get("kind") or "").strip()
        if kind != "replace_text":
            raise OfficeError(
                f"op #{number}: для pptx в MVP только replace_text "
                "(правки слайдов — фаза 2, TR-7.3a)")
        find = str(op.get("find") or "")
        replace = str(op.get("replace") or "")
        count = 0
        for slide in prs.slides:
            for shape in slide.shapes:
                if not getattr(shape, "has_text_frame", False):
                    continue
                for para in shape.text_frame.paragraphs:
                    for run in para.runs:
                        if find and find in run.text:
                            run.text = run.text.replace(find, replace)
                            count += 1
        applied.append({"op": kind, "runs": count})
    prs.save(str(path))
    return {"path": str(path), "kind": "pptx", "applied": applied,
            "warnings": []}


# ── создание ─────────────────────────────────────────────────────────────


def office_create(path: str, kind: str, content: dict) -> dict:
    """Создать новый документ: `docx` (из markdown), `xlsx` (таблицы), `pptx`."""
    kind = (kind or "").strip().lower()
    p = Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    if kind == "docx":
        return _create_docx(p, content)
    if kind == "xlsx":
        return _create_xlsx(p, content)
    if kind == "pptx":
        return _create_pptx(p, content)
    raise OfficeError(f"office_create: неизвестный вид документа {kind!r}")


_MD_TABLE_ROW = re.compile(r"^\s*\|(.+)\|\s*$")
_MD_TABLE_SEP = re.compile(r"^\s*\|[\s:\-|]+\|\s*$")


def _parse_markdown_table(lines: list[str], start: int) -> tuple[list[list[str]], int]:
    """Таблица markdown с позиции `start`: (строки, индекс за таблицей)."""
    rows: list[list[str]] = []
    index = start
    while index < len(lines):
        line = lines[index]
        match = _MD_TABLE_ROW.match(line)
        if match is None:
            break
        if not _MD_TABLE_SEP.match(line):
            rows.append([cell.strip() for cell in match.group(1).split("|")])
        index += 1
    return rows, index


def _create_docx(path: Path, content: dict) -> dict:
    import docx  # noqa: PLC0415

    markdown = str(content.get("markdown") or "")
    doc = docx.Document()
    lines = markdown.splitlines()
    index = 0
    tables = 0
    while index < len(lines):
        line = lines[index]
        if not line.strip():
            index += 1
            continue
        if _MD_TABLE_ROW.match(line):
            rows, index = _parse_markdown_table(lines, index)
            if rows:
                table = doc.add_table(rows=len(rows), cols=max(len(r) for r in rows))
                table.style = "Table Grid"
                for row_index, row in enumerate(rows):
                    for col_index, value in enumerate(row):
                        if col_index < len(table.rows[row_index].cells):
                            table.rows[row_index].cells[col_index].text = value
                tables += 1
            continue
        if line.startswith("### "):
            doc.add_heading(line[4:].strip(), level=3)
        elif line.startswith("## "):
            doc.add_heading(line[3:].strip(), level=2)
        elif line.startswith("# "):
            doc.add_heading(line[2:].strip(), level=1)
        elif line.lstrip().startswith(("- ", "* ")):
            doc.add_paragraph(line.lstrip()[2:].strip(), style="List Bullet")
        else:
            doc.add_paragraph(line.strip())
        index += 1
    doc.save(str(path))
    return {"path": str(path), "kind": "docx", "tables": tables,
            "paragraphs": len(doc.paragraphs), "warnings": []}


def _create_xlsx(path: Path, content: dict) -> dict:
    import openpyxl  # noqa: PLC0415

    wb = openpyxl.Workbook()
    wb.remove(wb.active)
    sheets = content.get("sheets") or []
    if not sheets:
        # markdown-таблицы: каждая таблица — отдельный лист
        markdown = str(content.get("markdown") or "")
        lines = markdown.splitlines()
        index = 0
        number = 1
        while index < len(lines):
            if _MD_TABLE_ROW.match(lines[index]):
                rows, index = _parse_markdown_table(lines, index)
                sheets.append({"name": f"Лист{number}", "rows": rows})
                number += 1
                continue
            index += 1
    if not sheets:
        raise OfficeError("office_create(xlsx): нет таблиц в content")
    for sheet in sheets:
        ws = wb.create_sheet(title=str(sheet.get("name") or "Лист"))
        for row in sheet.get("rows") or []:
            ws.append(list(row))
    wb.save(str(path))
    return {"path": str(path), "kind": "xlsx", "sheets": len(wb.worksheets),
            "warnings": []}


def _create_pptx(path: Path, content: dict) -> dict:
    try:
        from pptx import Presentation  # noqa: PLC0415
    except ImportError as exc:  # pragma: no cover
        raise OfficeError(
            "python-pptx не установлен в venv анонимизации: "
            "powershell -File installers/build_anonymize_sidecar.ps1 -Profile cpu"
        ) from exc
    slides = content.get("slides") or []
    if not slides:
        raise OfficeError("office_create(pptx): нет слайдов в content")
    prs = Presentation()
    layout = prs.slide_layouts[1]  # «Заголовок и объект» без оформления
    for slide in slides:
        page = prs.slides.add_slide(layout)
        page.shapes.title.text = str(slide.get("title") or "")
        body = page.placeholders[1].text_frame
        bullets = list(slide.get("bullets") or [])
        body.text = str(bullets[0]) if bullets else ""
        for bullet in bullets[1:]:
            body.add_paragraph().text = str(bullet)
    prs.save(str(path))
    return {"path": str(path), "kind": "pptx", "slides": len(prs.slides),
            "warnings": []}
