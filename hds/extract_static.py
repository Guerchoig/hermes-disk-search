"""MS Project (.mpp, через mpxj/Java) и картинки (EXIF + OCR)."""
import os

from .config import dig
from .extractors import seg, _ocr_pil_image, _tesseract_ready

MPP_EXTS = {".mpp", ".mpt", ".mpx"}
IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".webp", ".gif"}


def kind_for_ext_media(ext):
    if ext in MPP_EXTS:
        return "mpp"
    if ext in IMAGE_EXTS:
        return "image"
    return None


def extract_mpp(path, cfg):
    """MS Project через библиотеку mpxj (Java). Если её нет — пометка в индексе."""
    try:
        import jpype
        import mpxj  # noqa: F401
    except Exception as e:  # noqa: BLE001
        return [seg(
            "Файл MS Project %s. Текст не извлечён: установите пакет mpxj "
            "(pip install mpxj, требуется Java 11+). Детали: %s" % (os.path.basename(path), e)
        )]
    try:
        if not jpype.isJVMStarted():
            mpxj.startJvm()
        from org.mpxj.reader import UniversalProjectReader

        project = UniversalProjectReader().read(path)
        lines, segs = [], []
        for task in project.getTasks():
            name = task.getName()
            if not name:
                continue
            line = "%s; начало=%s; окончание=%s" % (name, task.getStart(), task.getFinish())
            try:
                res = []
                for ra in task.getResourceAssignments():
                    r = ra.getResource()
                    if r is not None and r.getName():
                        res.append(r.getName())
                if res:
                    line += "; ресурсы: " + ", ".join(res)
            except Exception:  # noqa: BLE001
                pass
            lines.append(line)
            if len(lines) >= 200:
                segs.append(seg("\n".join(lines)))
                lines = []
        if lines:
            segs.append(seg("\n".join(lines)))
        return segs or [seg("Пустой проект MS Project")]
    except Exception as e:  # noqa: BLE001
        return [seg("Файл MS Project %s: ошибка извлечения: %s" % (path, e))]


def extract_image(path, cfg):
    from PIL import Image

    img = Image.open(path)
    info = ["Изображение: %dx%d px, формат %s" % (img.width, img.height, img.format or "?")]
    try:
        exif = img.getexif()
        names = {0x0132: "дата", 0x0110: "камера", 0x8298: "автор", 0x9286: "описание"}
        for tag, label in names.items():
            if tag in exif and exif[tag]:
                info.append("%s: %s" % (label, str(exif[tag])[:200]))
    except Exception:  # noqa: BLE001
        pass
    texts = ["\n".join(info)]
    if dig(cfg, "index.ocr", True) and _tesseract_ready(cfg):
        try:
            ocr = _ocr_pil_image(img, cfg)
            if ocr and ocr.strip():
                texts.append("Текст на изображении (OCR):\n" + ocr.strip())
        except Exception as e:  # noqa: BLE001
            texts.append("OCR не удался: %s" % e)
    return [seg("\n\n".join(texts))]


def extract_dispatch_static(path, cfg, kind):
    if kind == "mpp":
        return kind, extract_mpp(path, cfg)
    if kind == "image":
        return kind, extract_image(path, cfg)
    return None, []