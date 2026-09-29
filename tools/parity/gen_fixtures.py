"""Генератор фикстур паритет-harness (W0, §4 п.2, §12.1 плана MIGRATION_PLAN_RUST.md).

Создаёт tools/parity/fixtures/ — набор, покрывающий ВСЕ ветки извлечения и чанкинга:
  md с 3 уровнями заголовков; docx со стилями Heading 1–3 + таблица;
  xlsx с несколькими листами и обрезкой > 5000 строк; pptx с таблицей;
  pdf многостраничный + скан без текста (OCR); csv/лог > max_chunks;
  картинка с EXIF и без; .mpp с задачами и ресурсами (копия реального файла);
  короткие wav/mp4.

Имена файлов — кириллические (заодно проверяем не-ASCII пути).

Запуск:  .venv/Scripts/python.exe tools/parity/gen_fixtures.py [--mpp-src PATH]
"""
import argparse
import os
import subprocess
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))          # корень проекта
FIX = os.path.join(BASE, "fixtures")

sys.path.insert(0, ROOT)


def _font_ru():
    """Cyrillic-шрифт Windows для PIL (scan-pdf и подписи картинок)."""
    import PIL.ImageFont

    for name in ("arial.ttf", "calibri.ttf", "segoeui.ttf"):
        p = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts", name)
        if os.path.exists(p):
            try:
                return PIL.ImageFont.truetype(p, 26)
            except Exception:  # noqa: BLE001
                continue
    return PIL.ImageFont.load_default()


def gen_md():
    p = os.path.join(FIX, "инструкция_документооборот.md")
    with open(p, "w", encoding="utf-8") as f:
        f.write(
            "# Работа с 1С:Документооборот\n\n"
            "## Настройки согласования\n\n"
            "В системе 1С:Документооборот настройка маршрутов согласования договоров "
            "выполняется администратором. Пороговые суммы согласования задаются в "
            "справочнике «Настройки отчёта».\n\n"
            "### Пороговые суммы\n\n"
            "Договоры до 100 000 рублей согласуются автоматически. Свыше порога — "
            "маршрут с юридической проверкой и подписью руководителя проекта.\n\n"
            "## Архив согласования\n\n"
            "Архив согласования хранится в базе docob-iris. Выгрузка документов "
            "выполняется еженедельно по расписанию планировщика.\n")
    return p


def gen_docx():
    from docx import Document

    d = Document()
    d.add_heading("Пояснительная записка проекта РТК", 1)
    d.add_paragraph(
        "На этапе предпроектного обследования было принято решение использовать "
        "1С:Документооборот совместно с MS Project для управления календарными планами. "
        "Ответственный: Саша. Сроки: 2024-2026.")
    d.add_heading("Техническое задание", 2)
    d.add_paragraph(
        "Техническое задание включает интеграцию с 1С:ERP, загрузку реестра систем "
        "и отчётность по порогам согласования договоров подряда.")
    d.add_heading("Приложения", 3)
    d.add_paragraph("Приложение А — реестр систем. Приложение Б — регламент СОУД.")
    t = d.add_table(rows=3, cols=3)
    hdr = t.rows[0].cells
    hdr[0].text = "Проект"
    hdr[1].text = "Система"
    hdr[2].text = "Статус"
    row1 = t.rows[1].cells
    row1[0].text = "ИРИС"
    row1[1].text = "1С:Документооборот"
    row1[2].text = "внедрен"
    row2 = t.rows[2].cells
    row2[0].text = "ЦС"
    row2[1].text = "Confluence"
    row2[2].text = "внедрена"
    p = os.path.join(FIX, "записка_ртк_полная.docx")
    d.save(p)
    return p


def gen_xlsx():
    import openpyxl

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Системы"
    ws.append(["Проект", "Система", "Статус"])
    ws.append(["ИРИС", "1С:Документооборот", "внедрен"])
    ws.append(["РТК", "1С:Документооборот", "внедрен"])
    ws2 = wb.create_sheet("Комплектующие")
    ws2.append(["Артикул", "Наименование", "Количество"])
    ws2.append(["A-001", "Датчик температуры", "12"])
    ws2.append(["A-002", "Контроллер склада", "3"])
    # лист с > 5000 строк — проверка обрезки в extract_xlsx (r >= 5000)
    ws3 = wb.create_sheet("Большой реестр")
    for i in range(1, 5200):
        ws3.append(["строка-%05d" % i, "позиция реестра %d" % i, "значение %d" % i])
    p = os.path.join(FIX, "реестр_систем_полный.xlsx")
    wb.save(p)
    return p


def gen_pptx():
    from pptx import Presentation

    prs = Presentation()
    s1 = prs.slides.add_slide(prs.slide_layouts[1])
    s1.shapes.title.text = "Презентация проекта ЦС"
    s1.placeholders[1].text = (
        "В проекте ЦС (Цифровой склад) использовался 1С:Документооборот для "
        "согласования КД и документации ЕСКД.")
    s2 = prs.slides.add_slide(prs.slide_layouts[5])
    s2.shapes.title.text = "Реестр систем"
    left, top = s2.shapes.title.left, s2.shapes.title.top + 1000000
    tbl = s2.shapes.add_table(3, 2, left, top, 4000000, 1500000).table
    tbl.cell(0, 0).text = "Проект"
    tbl.cell(0, 1).text = "Система"
    tbl.cell(1, 0).text = "ИРИС"
    tbl.cell(1, 1).text = "1С:Документооборот"
    tbl.cell(2, 0).text = "ЦС"
    tbl.cell(2, 1).text = "Confluence"
    p = os.path.join(FIX, "презентация_цс_с_таблицей.pptx")
    prs.save(p)
    return p


def gen_pdf_multipage():
    import fitz

    doc = fitz.open()
    for i in range(1, 7):
        page = doc.new_page()
        page.insert_text((72, 72), "Акт сверки взаиморасчетов № %d" % i,
                         fontsize=14, fontname="china-s")
        page.insert_text((72, 100),
                         "Страница %d акта. Использовался 1С:Документооборот версии 2.1 "
                         "для регламента СОУД и маршрутов согласования проектной "
                         "документации." % i,
                         fontsize=10, fontname="china-s")
        page.insert_text((72, 130),
                         "Порог согласования договоров подряда: %d рублей." % (i * 10000),
                         fontsize=10, fontname="china-s")
    p = os.path.join(FIX, "акты_сверки_многостраничный.pdf")
    doc.save(p)
    doc.close()
    return p


def gen_pdf_scan():
    """PDF из «отсканированных» страниц: текст существует только как картинка
    (проверка ветки OCR пустых страниц: extract_pdf, len(txt) < 40)."""
    import fitz
    from PIL import Image, ImageDraw

    font = _font_ru()
    pages = [
        ["Скан договора подряда № 15-ДП", "Заказчик: ООО Спринт", "Подрядчик: ЭПУ-Сервис",
         "Порог согласования: 250000 рублей", "Регламент СОУД применён"],
        ["Скан спецификации ТМЦ", "Позиция 1: контроллер склада", "Позиция 2: датчик температуры",
         "Итого: 15 позиций", "Подписи сторон: есть"],
    ]
    doc = fitz.open()
    for lines_page in pages:
        img = Image.new("RGB", (1000, 400), "white")
        dr = ImageDraw.Draw(img)
        y = 30
        for ln in lines_page:
            dr.text((40, y), ln, fill="black", font=font)
            y += 55
        tmp = os.path.join(FIX, "_tmp_scan.png")
        img.save(tmp)
        page = doc.new_page()
        page.insert_image(fitz.Rect(50, 50, 545, 270), filename=tmp)
        os.remove(tmp)
    p = os.path.join(FIX, "скан_договора_без_текста.pdf")
    doc.save(p)
    doc.close()
    return p


def gen_big_csv():
    """CSV длиннее max_chunks (3000): обрезка в indexer.process_file."""
    p = os.path.join(FIX, "большой_реестр.csv")
    with open(p, "w", encoding="utf-8") as f:
        f.write("проект;система;год;описание\n")
        for i in range(1, 61000):
            if i == 45000:
                f.write("МАРКЕР-ГЛУБОКО;секция-за-пределом-обрезки;2026;эта запись должна "
                        "попасть за границу 3000 чанков и быть обрезана\n")
            else:
                f.write("строка-%05d;система-%d;202%d;описание позиции реестра оборудования "
                        "склада номер %d\n" % (i, i % 20, i % 6, i))
    return p


def gen_big_log():
    """Лог длиннее max_chunks (3000 чанков)."""
    p = os.path.join(FIX, "журнал_обработки.log")
    with open(p, "w", encoding="utf-8") as f:
        for i in range(1, 42000):
            lvl = "INFO" if i % 10 else "ERROR"
            if i == 38000:
                f.write("2026-09-29 10:00:00 ERROR САЙРА-МАРКЕР запись за границей "
                        "обрезки чанков %d\n" % i)
            else:
                f.write("2026-09-%02d 10:%02d:%02d %s обработка записи %d код ошибки %d\n"
                        % (i % 28 + 1, i % 60, i % 60, lvl, i, i % 7))
    return p


def gen_images():
    from PIL import Image, ImageDraw

    font = _font_ru()
    # с EXIF
    img = Image.new("RGB", (480, 360), "ivory")
    dr = ImageDraw.Draw(img)
    dr.text((30, 40), "Накладная № 42", fill="black", font=font)
    dr.text((30, 90), "Дата 01.09.2026 склад N 3", fill="black", font=font)
    exif = Image.Exif()
    exif[0x0110] = "TestCam-42"            # камера
    exif[0x0132] = "2026:09:01 12:00:00"   # дата
    exif[0x8298] = "Саша"                  # автор
    p1 = os.path.join(FIX, "накладная_с_exif.jpg")
    img.save(p1, "JPEG", exif=exif.tobytes())
    # без EXIF: «фото» цветов (проверка CLIP-ветки: папка + содержание)
    img2 = Image.new("RGB", (480, 360), "white")
    dr = ImageDraw.Draw(img2)
    for cx, cy, col in ((120, 120, "red"), (240, 160, "yellow"), (360, 120, "blue")):
        dr.ellipse((cx - 50, cy - 50, cx + 50, cy + 50), fill=col)
    dr.rectangle((0, 280, 480, 360), fill="green")
    p2 = os.path.join(FIX, "цветы_без_exif.jpg")
    img2.save(p2, "JPEG")
    return p1, p2


def gen_media():
    """Короткие wav/mp4 через ffmpeg; речевые — обрезки файлов из test_data."""
    import shutil

    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        print("[skip] ffmpeg не найден — медиа-фикстуры не созданы")
        return []
    made = []
    wav = os.path.join(FIX, "тон_3сек.wav")
    subprocess.run([ffmpeg, "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
                    "-ar", "16000", "-ac", "1", wav],
                   capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW)
    made.append(wav)
    mp4 = os.path.join(FIX, "видео_заставка_2сек.mp4")
    subprocess.run([ffmpeg, "-y", "-f", "lavfi",
                    "-i", "testsrc=duration=2:size=320x240:rate=15", mp4],
                   capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW)
    made.append(mp4)
    td = os.path.join(ROOT, "test_data")
    for src_name, dst_name, dur in (("jfk.wav", "речь_jfk_обрезок.wav", 4),
                                    ("speech_2min.wav", "речь_русская_обрезок.wav", 5)):
        src = os.path.join(td, src_name)
        if not os.path.exists(src):
            continue
        dst = os.path.join(FIX, dst_name)
        subprocess.run([ffmpeg, "-y", "-t", str(dur), "-i", src,
                        "-ar", "16000", "-ac", "1", dst],
                       capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW)
        made.append(dst)
    return made


def gen_mpp(src):
    """Копия реального .mpp (генерировать без Java/mpxj нельзя)."""
    if not src or not os.path.exists(src):
        print("[skip] .mpp: реальный файл не передан (--mpp-src); ветка mpp "
              "покрывается реальным деревом (полигон §12.3)")
        return None
    dst = os.path.join(FIX, "план_проекта_копия.mpp")
    with open(src, "rb") as a, open(dst, "wb") as b:
        b.write(a.read())
    return dst


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mpp-src", default=os.environ.get("HDS_PARITY_MPP", ""),
                    help="путь к реальному .mpp для копирования в фикстуры")
    args = ap.parse_args()
    os.makedirs(FIX, exist_ok=True)
    made = [gen_md(), gen_docx(), gen_xlsx(), gen_pptx(),
            gen_pdf_multipage(), gen_pdf_scan(), gen_big_csv(), gen_big_log()]
    made.extend(gen_images())
    made.extend(gen_media())
    mpp = gen_mpp(args.mpp_src)
    if mpp:
        made.append(mpp)
    made = [m for m in made if m]
    print("Созданы фикстуры (%d) в %s:" % (len(made), FIX))
    total = 0
    for m in sorted(made):
        sz = os.path.getsize(m)
        total += sz
        print("  - %-42s %10.1f КБ" % (os.path.basename(m), sz / 1024))
    print("Итого: %.1f МБ" % (total / 1048576))


if __name__ == "__main__":
    main()