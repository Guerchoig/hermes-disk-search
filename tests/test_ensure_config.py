"""Тесты устойчивости к отсутствующим/битым настройкам: ensure_config, safe-UI."""
import os
import sys
import tempfile
import unittest

from helpers import write_config  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds.config import config_path, ensure_config, load  # noqa: E402
from hds import ui_server  # noqa: E402


class EnsureConfigTests(unittest.TestCase):
    """РЕГРЕССИЯ: без config.yaml UI и CLI падали с FileNotFoundError."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-ensure-")
        self.old = os.environ.get("HDS_CONFIG")
        os.environ["HDS_CONFIG"] = os.path.join(self.tmp, "config.yaml")
        self.addCleanup(self._restore)

    def _restore(self):
        if self.old is None:
            os.environ.pop("HDS_CONFIG", None)
        else:
            os.environ["HDS_CONFIG"] = self.old

    def test_creates_default_config(self):
        self.assertFalse(os.path.exists(config_path()))
        self.assertTrue(ensure_config())
        cfg = load()
        self.assertEqual(cfg["embedding"]["dim"], 1024)
        self.assertEqual(cfg["index"]["roots"], [os.path.expanduser("~")])
        self.assertIn("exclude_dirs", cfg["index"])
        self.assertTrue(os.path.exists(config_path()))

    def test_no_overwrite_existing(self):
        with open(config_path(), "w", encoding="utf-8") as f:
            f.write("index:\n  roots: ['X:\\custom']\n")
        self.assertFalse(ensure_config())
        self.assertEqual(load()["index"]["roots"], ["X:\\custom"])


class SafeUiTests(unittest.TestCase):
    """UI обязан открываться при любых настройках: ошибки — в JSON, не в падение."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-safeui-")
        self.cfg = write_config(self.tmp)
        with open(self.cfg, encoding="utf-8-sig") as f:
            self.cfg_text = f.read()

        def _restore():
            # Не удаляем файл: последующие модули (test_extractors) читают
            # HDS_CONFIG до своего setUp. Возвращаем валидное содержимое.
            with open(self.cfg, "w", encoding="utf-8") as f:
                f.write(self.cfg_text)
        self.addCleanup(_restore)

    def test_safe_cfg_ok(self):
        cfg, err = ui_server._safe_cfg()
        self.assertEqual(err, "")
        self.assertTrue(cfg)

    def test_safe_cfg_broken_yaml(self):
        with open(self.cfg, "w", encoding="utf-8") as f:
            f.write("index: [broken\n")
        cfg, err = ui_server._safe_cfg()
        self.assertEqual(cfg, {})
        self.assertIn("не читается", err)

    def test_db_stats_survives_missing_db(self):
        info = ui_server._db_stats()
        self.assertIn("path", info)
        self.assertIn("size_mb", info)

    def test_model_status_keys(self):
        st = ui_server._model_status()
        for k in ("gguf_path", "gguf_ready", "downloading", "progress",
                  "msg", "server_ok", "model_loaded"):
            self.assertIn(k, st)
        self.assertIsInstance(st["gguf_ready"], bool)
        self.assertIsInstance(st["downloading"], bool)


class DiagTests(unittest.TestCase):
    """UI/CLI-диагностика компонентов: структура, изоляция от сети."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-diag-")
        self.cfg = write_config(self.tmp)  # base_url 127.0.0.1:1 — мгновенный отказ сети
        self.addCleanup(lambda: os.path.exists(self.cfg) and os.remove(self.cfg))

    def test_run_checks_structure(self):
        from hds.diag import run_checks
        checks = run_checks()
        ids = {c["id"] for c in checks}
        for expected in ("db", "roots", "chat", "emb", "ocr", "ffmpeg", "whisper", "mpp"):
            self.assertIn(expected, ids)
        for c in checks:
            self.assertIn(c["status"], ("ok", "warn", "fail"))
            self.assertTrue(c["title"])
            if c["status"] != "ok":
                self.assertTrue(c["fix"], "у проблемы должна быть подсказка: %s" % c["id"])

    def test_db_fail_on_missing_drive(self):
        from hds.diag import has_failures, run_checks
        with open(self.cfg, "w", encoding="utf-8") as f:
            f.write("index:\n"
                    "  roots: ['Q:\\data']\n"
                    "embedding:\n"
                    "  dim: 8\n"
                    "  base_url: 'http://127.0.0.1:1/v1'\n"
                    "db_path: 'Q:\\missing\\index.db'\n"
                    "chat:\n"
                    "  base_url: 'http://127.0.0.1:1/v1'\n")
        checks = run_checks()
        db = next(c for c in checks if c["id"] == "db")
        self.assertEqual(db["status"], "fail")
        self.assertTrue(has_failures(checks))
        roots = next(c for c in checks if c["id"] == "roots")
        self.assertEqual(roots["status"], "warn")


class PickDeviceTests(unittest.TestCase):
    """Авто-детекция устройства транскрипции (Windows: CUDA/CPU; macOS: Metal/CPU)."""

    def test_cpu_explicit(self):
        from hds.extract_av import _pick_device
        self.assertEqual(_pick_device({"index": {"whisper_device": "cpu"}}, platform="win32"),
                         ("cpu", "int8"))

    def test_cpu_float32_kept(self):
        from hds.extract_av import _pick_device
        self.assertEqual(
            _pick_device({"index": {"whisper_device": "cpu", "whisper_compute": "float32"}},
                         platform="win32"),
            ("cpu", "float32"))

    def test_cuda_on_mac_falls_back_to_cpu(self):
        # mlx-whisper на Windows не импортируется -> Metal/CUDA на mac => CPU int8
        from hds.extract_av import _pick_device
        self.assertEqual(_pick_device({"index": {"whisper_device": "cuda"}}, platform="darwin"),
                         ("cpu", "int8"))
        self.assertEqual(_pick_device({"index": {"whisper_device": "metal"}}, platform="darwin"),
                         ("cpu", "int8"))

    def test_vulkan_not_supported_falls_back(self):
        from hds.extract_av import _pick_device
        dev, comp = _pick_device({"index": {"whisper_device": "vulkan"}}, platform="win32")
        self.assertIn(dev, ("cuda", "cpu"))
        self.assertEqual(comp, "float16" if dev == "cuda" else "int8")

    def test_metal_on_windows_falls_back(self):
        from hds.extract_av import _pick_device
        dev, comp = _pick_device({"index": {"whisper_device": "metal"}}, platform="win32")
        self.assertIn(dev, ("cuda", "cpu"))

    def test_auto_detects_valid_device(self):
        # На машине с CUDA -> cuda/float16, без CUDA -> cpu/int8; оба варианта валидны
        from hds.extract_av import _pick_device
        dev, comp = _pick_device({"index": {"whisper_device": "auto"}}, platform="win32")
        self.assertIn((dev, comp), (("cuda", "float16"), ("cpu", "int8")))

    def test_default_is_auto(self):
        from hds.extract_av import _pick_device
        dev, comp = _pick_device({}, platform="win32")
        self.assertIn((dev, comp), (("cuda", "float16"), ("cpu", "int8")))


class PickDeviceVulkanTests(unittest.TestCase):
    """Цепочка CUDA → Vulkan (whisper.cpp) → CPU для AMD/Intel на Windows."""

    def _pick(self, device, cpp, cuda_count):
        from hds.extract_av import _pick_device
        return _pick_device({"index": {"whisper_device": device}},
                            platform="win32", cpp_available=cpp, cuda_count=cuda_count)

    def test_vulkan_explicit_backend_ready(self):
        self.assertEqual(self._pick("vulkan", True, 0), ("vulkan", "ggml"))

    def test_amd_alias(self):
        self.assertEqual(self._pick("amd", True, 0), ("vulkan", "ggml"))

    def test_auto_no_cuda_backend_ready(self):
        self.assertEqual(self._pick("auto", True, 0), ("vulkan", "ggml"))

    def test_auto_no_cuda_no_backend(self):
        self.assertEqual(self._pick("auto", False, 0), ("cpu", "int8"))

    def test_auto_cuda_preferred_over_vulkan(self):
        self.assertEqual(self._pick("auto", True, 2), ("cuda", "float16"))

    def test_vulkan_missing_backend_falls_back_to_cpu(self):
        self.assertEqual(self._pick("vulkan", False, 0), ("cpu", "int8"))


class WhisperCppTests(unittest.TestCase):
    """Бэкенд whisper.cpp (Vulkan для AMD/Intel): парсинг JSON, поиск бинарника."""

    def test_load_transcription(self):
        import json
        import tempfile

        from hds import whisper_cpp
        sample = {"transcription": [
            {"offsets": {"from": 0, "to": 1500}, "text": " Привет, мир. "},
            {"offsets": {"from": 1500, "to": 3200}, "text": " Как дела?"},
        ]}
        p = os.path.join(tempfile.mkdtemp(prefix="hds-wcpp-"), "out.json")
        with open(p, "w", encoding="utf-8") as f:
            json.dump(sample, f)
        segs, info = whisper_cpp._load_transcription(p)
        self.assertEqual(len(segs), 2)
        self.assertEqual(segs[0].start, 0.0)
        self.assertEqual(segs[0].end, 1.5)
        self.assertEqual(segs[0].text, "Привет, мир.")
        self.assertEqual(info.duration, 3.2)

    def test_find_exe_recursive(self):
        import tempfile

        from hds import whisper_cpp
        tmp = tempfile.mkdtemp(prefix="hds-wcpp-exe-")
        nested = os.path.join(tmp, "build", "bin", "Release")
        os.makedirs(nested)
        open(os.path.join(nested, "whisper-cli.exe"), "wb").close()
        cfg = {"index": {"whisper_cpp_dir": tmp}}
        self.assertEqual(whisper_cpp.find_exe(cfg),
                         os.path.join(nested, "whisper-cli.exe"))
        self.assertFalse(whisper_cpp.available(cfg))  # весов нет — не готов


if __name__ == "__main__":
    unittest.main()
