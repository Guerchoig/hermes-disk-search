"""Текстовые образы `pdf`/`pptx`/`mpp` для анонимной зоны (D18).

Агент получает не анонимный PDF/PPTX (честно пересобрать дорого), а
**текстовый образ** `<имя>.<ext>.md`: текст извлекается `hds_extract`
(в воркере — собственными средствами или портативным CPython), затем
анонимизируется как markdown через `anonymize_text` (TR-1.7) и пишется в
рабочую папку проекта. Образ **только для чтения** — запись в него
блокирует `hds-guard` (A3, TR-7.3a).

Разметка образа сохраняет структуру источника:

* `pdf` — `<!-- страница N -->` перед текстом страницы;
* `pptx` — `## Слайд N`;
* `mpp` — текст из `hds_extract` (задачи MS Project) как есть:
  в A1 образ строится по готовому тексту, разбор задач — A4.

Извлечение `mpp` без `hds_extract` невозможно (`mpxj` + Java живут в
портативном CPython извлечения): такой запрос возвращает `extract_required`,
и Rust передаёт текст, полученный от `hds_extract`.
"""
from __future__ import annotations

import logging
import os
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Optional

logger = logging.getLogger("hds_anonymize.text_image")

PDF_EXTS = {".pdf"}
PPTX_EXTS = {".pptx"}
MPP_EXTS = {".mpp", ".mpt", ".mpx"}
IMAGE_EXTS = PDF_EXTS | PPTX_EXTS | MPP_EXTS

# Маркер образа: hds-agent/<P>/<artifact_id> (TR-7.4); HTML-комментарий
# скрыт при рендере markdown и виден агенту в read_files
MARKER_RE = re.compile(r"^<!--\s*hds-agent/([^/\s]+)(?:/([^\s]+))?\s*-->$")


class ExtractRequired(RuntimeError):
    """Текст нужно получить через `hds_extract` (pdf/pptx/mpp без библиотек)."""


def image_path(out_dir: str, in_path: str) -> Path:
    """`<имя>.<ext>.md` — образ рядом с именем оригинала (D18)."""
    source = Path(in_path)
    return Path(out_dir) / f"{source.name}.md"


def marker_line(project_id: str, artifact_id: Optional[str] = None) -> str:
    """Строка маркера образа: `<!-- hds-agent/P0001/123 -->`."""
    project_id = (project_id or "").strip()
    artifact_id = (artifact_id or "").strip()
    if artifact_id:
        return f"<!-- hds-agent/{project_id}/{artifact_id} -->"
    return f"<!-- hds-agent/{project_id} -->"


def read_marker(text: str) -> Optional[tuple[str, Optional[str]]]:
    """Разобрать маркер образа: `(project_id, artifact_id)` или None."""
    lines = (text or "").lstrip().splitlines()
    if not lines:
        return None
    match = MARKER_RE.match(lines[0].strip())
    if match is None:
        return None
    return match.group(1), match.group(2)


def is_text_image(path: str) -> bool:
    """Имя файла — текстовый образ (`*.pdf.md`, `*.pptx.md`, `*.mpp.md`)."""
    name = Path(path).name.lower()
    return any(name.endswith(f"{ext}.md") for ext in IMAGE_EXTS)


def build_text_image(
    text: str,
    in_path: str,
    project_id: str,
    artifact_id: Optional[str] = None,
) -> str:
    """Собрать текст образа: маркер + заголовок + текст источника."""
    source = Path(in_path)
    header = marker_line(project_id, artifact_id)
    return f"{header}\n# {source.name}\n\n{text.strip()}\n"


def write_text_image(path: str, body: str) -> None:
    """Атомарно записать образ (tmp + os.replace)."""
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_name(target.name + ".tmp")
    try:
        tmp.write_text(body, encoding="utf-8")
        os.replace(tmp, target)
    finally:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass


# ── извлечение текста ───────────────────────────────────────────────────


def _portable_python() -> Optional[Path]:
    """Портативный CPython извлечения (`sidecar/python/cpython-*/python.exe`)."""
    root = Path(__file__).resolve().parents[2]
    base = root / "sidecar" / "python"
    if not base.is_dir():
        return None
    candidates = sorted(base.glob("cpython-*/python.exe"), reverse=True)
    return candidates[0] if candidates else None


def _run_portable(code: str, src: str, suffix: str) -> str:
    """Выполнить извлечение портативным CPython (PyMuPDF/python-pptx/mpxj)."""
    python = _portable_python()
    if python is None:
        raise ExtractRequired(
            "нет портативного CPython извлечения (sidecar/python): "
            "передайте текст из hds_extract"
        )
    handle, out_path = tempfile.mkstemp(suffix=suffix, text=False)
    os.close(handle)
    try:
        subprocess.run(
            [str(python), "-c", code, src, out_path],
            check=True,
            capture_output=True,
            timeout=600,
        )
        return Path(out_path).read_text(encoding="utf-8")
    except subprocess.CalledProcessError as exc:
        message = (exc.stderr or b"").decode("utf-8", "replace")[-500:]
        raise ExtractRequired(f"извлечение не удалось: {message}") from exc
    finally:
        try:
            os.remove(out_path)
        except OSError:
            pass


_PDF_CODE = (
    "import sys, pymupdf\n"
    "doc = pymupdf.open(sys.argv[1])\n"
    "parts = []\n"
    "for number, page in enumerate(doc, start=1):\n"
    "    parts.append('\\n\\n<!-- страница %d -->\\n\\n%s'\n"
    "                 % (number, page.get_text('text').strip()))\n"
    "doc.close()\n"
    "open(sys.argv[2], 'w', encoding='utf-8').write(''.join(parts))\n"
)

_PPTX_CODE = (
    "import sys\n"
    "from pptx import Presentation\n"
    "prs = Presentation(sys.argv[1])\n"
    "out = []\n"
    "for index, slide in enumerate(prs.slides, start=1):\n"
    "    lines = ['', '', '## Слайд %d' % index]\n"
    "    for shape in slide.shapes:\n"
    "        if not getattr(shape, 'has_text_frame', False):\n"
    "            continue\n"
    "        text = shape.text_frame.text.strip()\n"
    "        if text:\n"
    "            lines.append(text)\n"
    "    out.append('\\n'.join(lines))\n"
    "open(sys.argv[2], 'w', encoding='utf-8').write('\\n'.join(out))\n"
)


def _pdf_text(path: Path) -> str:
    try:
        import pymupdf  # noqa: PLC0415
    except ImportError:
        return _run_portable(_PDF_CODE, str(path), ".txt")
    doc = pymupdf.open(str(path))
    try:
        return "".join(
            f"\n\n<!-- страница {number} -->\n\n{page.get_text('text').strip()}"
            for number, page in enumerate(doc, start=1)
        )
    finally:
        doc.close()


def _pptx_text(path: Path) -> str:
    try:
        from pptx import Presentation  # noqa: PLC0415
    except ImportError:
        return _run_portable(_PPTX_CODE, str(path), ".txt")
    prs = Presentation(str(path))
    out = []
    for index, slide in enumerate(prs.slides, start=1):
        lines = ["", "", f"## Слайд {index}"]
        for shape in slide.shapes:
            if not getattr(shape, "has_text_frame", False):
                continue
            text = shape.text_frame.text.strip()
            if text:
                lines.append(text)
        out.append("\n".join(lines))
    return "\n".join(out)


def extract_text(path: str, ext: Optional[str] = None) -> str:
    """Текст источника: PyMuPDF/python-pptx, иначе портативный CPython.

    `mpp` без `hds_extract` не извлекается — `ExtractRequired` (mpxj/Java).
    """
    p = Path(path)
    if not p.is_file():
        raise FileNotFoundError(f"Файл не найден: {p}")
    suffix = (ext or p.suffix).lower()
    if suffix in PDF_EXTS:
        return _pdf_text(p)
    if suffix in PPTX_EXTS:
        return _pptx_text(p)
    if suffix in MPP_EXTS:
        raise ExtractRequired(
            "MS Project извлекает hds_extract (mpxj + Java): передайте text"
        )
    raise ExtractRequired(f"не текстовый образ: {suffix or p.name}")
