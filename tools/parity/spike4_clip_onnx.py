"""Спайк 4 (W0 §4 п.6): CLIP в ONNX Runtime — экспорт и численный паритет.

Экспортирует оба энкодера (vision: clip-ViT-B-32, text: clip-ViT-B-32-multilingual-v1)
в ONNX и сверяет косинус с текущим sentence-transformers. Критерий спайка: cos ≥ 0,999
и RSS процесса с загруженными моделями ≤ 400 МБ.
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


def cosine(a, b):
    import numpy as np

    a, b = np.asarray(a, dtype="float32"), np.asarray(b, dtype="float32")
    return float(a.dot(b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-12))


def export_vision():
    import torch
    from sentence_transformers import SentenceTransformer

    st = SentenceTransformer("clip-ViT-32" if False else "clip-ViT-B-32", device="cpu")
    clip_model = st[0].model.to("cpu")             # transformers CLIPModel
    vision = clip_model.vision_model
    proj = clip_model.visual_projection
    os.makedirs(os.path.join(MODELS, "vision"), exist_ok=True)
    path = os.path.join(MODELS, "vision", "clip_vision.onnx")

    class Wrap(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.vision, self.proj = vision, proj

        def forward(self, pixel_values):
            out = self.vision(pixel_values=pixel_values)
            return self.proj(out.pooler_output)

    dummy = torch.randn(1, 3, 224, 224)
    if not os.path.exists(path):                   # кэш: повторный прогон не экспортирует
        torch.onnx.export(Wrap().eval(), (dummy,), path, input_names=["pixel_values"],
                          output_names=["image_embeds"], opset_version=17,
                          dynamo=False,
                          dynamic_axes={"pixel_values": {0: "batch"}})
    return st, path


def export_text():
    import torch
    from sentence_transformers import SentenceTransformer

    st = SentenceTransformer("sentence-transformers/clip-ViT-B-32-multilingual-v1",
                             device="cpu")
    auto = st[0].auto_model.to("cpu")               # XLMRobertaModel
    os.makedirs(os.path.join(MODELS, "text"), exist_ok=True)
    path = os.path.join(MODELS, "text", "clip_text_xlmr.onnx")
    dummy = torch.ones(1, 16, dtype=torch.long)
    mask = torch.ones(1, 16, dtype=torch.long)
    if not os.path.exists(path):                   # кэш экспорта
        torch.onnx.export(auto.eval(), (dummy, mask), path,
                          input_names=["input_ids", "attention_mask"],
                          output_names=["last_hidden_state"], opset_version=17,
                          dynamo=False,
                          dynamic_axes={"input_ids": {0: "batch", 1: "seq"},
                                        "attention_mask": {0: "batch", 1: "seq"}})
    return st, path
def run_parity():
    import numpy as np
    import onnxruntime as ort
    from PIL import Image
    from sentence_transformers import SentenceTransformer

    res = {"models": {}}
    t0 = time.time()
    st_vis, vis_path = export_vision()
    res["models"]["vision"] = {"path": vis_path, "sec": round(time.time() - t0, 1),
                               "mb": round(os.path.getsize(vis_path) / 1048576, 1)}
    t1 = time.time()
    st_txt, txt_path = export_text()
    res["models"]["text"] = {"path": txt_path, "sec": round(time.time() - t1, 1),
                             "mb": round(os.path.getsize(txt_path) / 1048576, 1)}
    print("экспорт: vision %.1f МБ, text %.1f МБ"
          % (res["models"]["vision"]["mb"], res["models"]["text"]["mb"]))

    so = ort.SessionOptions()
    so.log_severity_level = 3
    sess_v = ort.InferenceSession(vis_path, so, providers=["CPUExecutionProvider"])
    sess_t = ort.InferenceSession(txt_path, so, providers=["CPUExecutionProvider"])

    # --- vision: картинки из фикстур ---
    imgs = [os.path.join(FIX, f) for f in sorted(os.listdir(FIX))
            if f.lower().endswith((".jpg", ".png"))][:5]
    # препроцессинг строго как в sentence-transformers: resize + center-crop + normalize
    try:
        proc = st_vis[0].processor.image_processor
        res["vision_preprocess"] = "sentence-transformers CLIPImageProcessor"
    except Exception:  # noqa: BLE001
        from transformers import CLIPImageProcessor

        proc = CLIPImageProcessor.from_pretrained("openai/clip-vit-base-patch32")
        res["vision_preprocess"] = "transformers CLIPImageProcessor (openai)"
    res["vision_model_id"] = getattr(getattr(st_vis[0], "model", None), "config", None) \
        and st_vis[0].model.config._name_or_path
    cos_v = []
    for p in imgs:
        img = Image.open(p).convert("RGB")
        x = np.asarray(proc(images=img, return_tensors="np")["pixel_values"],
                       dtype="float32")
        v_ort = sess_v.run(["image_embeds"], {"pixel_values": x})[0][0]
        v_st = st_vis.encode([img], normalize_embeddings=False)[0]
        cos_v.append(cosine(v_ort, v_st))
    res["vision"] = {"n": len(cos_v), "cos_min": round(min(cos_v), 6),
                     "cos_mean": round(sum(cos_v) / max(len(cos_v), 1), 6)}

    # --- text: русские запросы ---
    queries = ["цветы в вазе", "накладная со склада", "документооборот и договоры",
               "схема интеграции систем", "пороговые суммы согласования",
               "акт сверки взаиморасчетов", "презентация проекта", "фото букета",
               "контроллер склада", "техническое задание"]
    cos_t = []
    tok = st_txt.tokenizer
    for q in queries:
        enc = tok([q], padding=True, truncation=True, max_length=128, return_tensors="np")
        ids = enc["input_ids"].astype("int64")
        mask = enc["attention_mask"].astype("int64")
        hidden = sess_t.run(["last_hidden_state"], {"input_ids": ids,
                                                    "attention_mask": mask})[0]
        m = mask[..., None].astype("float32")
        pooled = (hidden * m).sum(axis=1) / np.clip(m.sum(axis=1), 1e-9, None)
        dense = st_txt[2]                                  # Dense(768 → 512)
        w = dense.linear.weight.detach().cpu().numpy()
        b = (dense.linear.bias.detach().cpu().numpy()
             if dense.linear.bias is not None
             else np.zeros(w.shape[0], dtype="float32"))
        v_ort = pooled @ w.T + b
        v_st = st_txt.encode([q], normalize_embeddings=False)[0]
        cos_t.append(cosine(v_ort[0], v_st))
    res["text"] = {"n": len(cos_t), "cos_min": round(min(cos_t), 6),
                   "cos_mean": round(sum(cos_t) / max(len(cos_t), 1), 6)}

    dims = (sess_v.get_outputs()[0].shape, sess_t.get_outputs()[0].shape)
    res["shapes"] = {"vision_out": str(dims[0]), "text_out": str(dims[1])}
    ok = res["vision"]["cos_min"] >= 0.999 and res["text"]["cos_min"] >= 0.999
    res["verdict"] = ("ПАРИТЕТ (cos >= 0,999 оба энкодера)" if ok else "РАСХОЖДЕНИЕ")
    return res


def main():
    res = {"generated": time.strftime("%Y-%m-%d %H:%M:%S")}
    try:
        res.update(run_parity())
        print("vision: cos_min=%s cos_mean=%s (n=%s)"
              % (res["vision"]["cos_min"], res["vision"]["cos_mean"], res["vision"]["n"]))
        print("text:   cos_min=%s cos_mean=%s (n=%s)"
              % (res["text"]["cos_min"], res["text"]["cos_mean"], res["text"]["n"]))
        print("ВЕРДИКТ: %s" % res["verdict"])
    except Exception as e:  # noqa: BLE001
        import traceback
        res["error"] = "%s: %s" % (type(e).__name__, e)
        res["traceback"] = traceback.format_exc()[-1500:]
        print("ОШИБКА спайка 4: %s" % res["error"])
    with open(os.path.join(OUT, "spike4_clip_onnx.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike4_clip_onnx.json")


if __name__ == "__main__":
    main()