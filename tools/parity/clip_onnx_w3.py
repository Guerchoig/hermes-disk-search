"""W3/§7: CLIP — экспорт text-ONNX (pooling+Dense) и golden-векторы для паритета Rust.

Vision-ONNX уже есть (спайк 4). Здесь:
  * экспортируем text-энкодер мультиязычной модели С pooling(mean)+Dense(768→512)
    внутрь графа (выход `text_embeds` [batch,512]) — Rust тогда не тащит веса Dense;
  * считаем эталонные нормализованные векторы sentence-transformers (vision по 2
    картинкам-фикстурам, text по 10 запросам) → out/clip_parity.json.

Запуск: .venv\\Scripts\\python.exe tools\\parity\\clip_onnx_w3.py
"""
import json
import os
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
MODELS = os.path.join(OUT, "clip_onnx")
FIX = os.path.join(BASE, "fixtures")
os.makedirs(MODELS, exist_ok=True)
sys.path.insert(0, ROOT)

TXT_ID = "sentence-transformers/clip-ViT-B-32-multilingual-v1"
VIS_ID = "sentence-transformers/clip-ViT-B-32"


def export_text_dense():
    import torch
    from sentence_transformers import SentenceTransformer

    st = SentenceTransformer(TXT_ID, device="cpu")
    auto = st[0].auto_model.to("cpu").eval()
    dense = st[2].to("cpu").eval()  # Dense(768 -> 512)

    class Wrap(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.auto, self.dense = auto, dense

        def forward(self, input_ids, attention_mask):
            h = self.auto(input_ids=input_ids, attention_mask=attention_mask).last_hidden_state
            m = attention_mask.unsqueeze(-1).float()
            pooled = (h * m).sum(1) / m.sum(1).clamp(min=1e-9)
            return self.dense.linear(pooled)

    path = os.path.join(MODELS, "text", "clip_text_dense.onnx")
    if not os.path.exists(path):
        ids = torch.ones(1, 16, dtype=torch.long)
        mask = torch.ones(1, 16, dtype=torch.long)
        torch.onnx.export(Wrap().eval(), (ids, mask), path,
                          input_names=["input_ids", "attention_mask"],
                          output_names=["text_embeds"], opset_version=17, dynamo=False,
                          dynamic_axes={"input_ids": {0: "batch", 1: "seq"},
                                        "attention_mask": {0: "batch", 1: "seq"}})
    return path


def main():
    import numpy as np
    from PIL import Image

    res = {"generated": time.strftime("%Y-%m-%d %H:%M:%S")}
    txt_path = export_text_dense()
    res["text_onnx"] = txt_path
    res["text_onnx_mb"] = round(os.path.getsize(txt_path) / 1048576, 1)

    from sentence_transformers import SentenceTransformer
    st_vis = SentenceTransformer(VIS_ID, device="cpu")
    st_txt = SentenceTransformer(TXT_ID, device="cpu")

    # tokenizer.json (Rust читает его тем же tokenizers-парсером)
    tok_json = None
    try:
        from huggingface_hub import hf_hub_download
        tok_json = hf_hub_download(TXT_ID, "tokenizer.json")
    except Exception:  # noqa: BLE001
        pass
    res["tokenizer_json"] = tok_json

    imgs = ["накладная_с_exif.jpg", "цветы_без_exif.jpg"]
    res["images"] = {}
    for name in imgs:
        p = os.path.join(FIX, name)
        img = Image.open(p).convert("RGB")
        v = st_vis.encode([img], normalize_embeddings=True)[0]
        res["images"][name] = [float(x) for x in v]

    queries = ["цветы в вазе", "накладная со склада", "документооборот и договоры",
               "схема интеграции систем", "пороговые суммы согласования",
               "акт сверки взаиморасчетов", "презентация проекта", "фото букета",
               "контроллер склада", "техническое задание"]
    res["queries"] = {}
    for q in queries:
        v = st_txt.encode([q], normalize_embeddings=True)[0]
        res["queries"][q] = [float(x) for x in v]

    with open(os.path.join(OUT, "clip_parity.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False)
    print("text_onnx:", txt_path, "mb:", res["text_onnx_mb"])
    print("tokenizer_json:", tok_json)
    print("images:", len(res["images"]), "queries:", len(res["queries"]))
    print("сохранено: out/clip_parity.json")


if __name__ == "__main__":
    main()
