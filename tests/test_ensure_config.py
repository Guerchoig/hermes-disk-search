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
        # Путь, который не существует и не может быть создан на текущей ОС:
        # Windows — несуществующий диск Q:, POSIX — каталог в корне без прав.
        if os.name == "nt":
            db_path, root = "Q:\\missing\\index.db", "Q:\\data"
        else:
            db_path, root = "/nonexistent_hds_test_dir/index.db", "/nonexistent_hds_test_dir"
        with open(self.cfg, "w", encoding="utf-8") as f:
            f.write("index:\n"
                    "  roots: ['%s']\n"
                    "embedding:\n"
                    "  dim: 8\n"
                    "  base_url: 'http://127.0.0.1:1/v1'\n"
                    "db_path: '%s'\n"
                    "chat:\n"
                    "  base_url: 'http://127.0.0.1:1/v1'\n"
                    % (root.replace("'", "''"), db_path.replace("'", "''")))
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
        # cpp_available=False явно: иначе тест зависит от окружения (если
        # whisper.cpp установлен на машине, детекция честно вернёт vulkan)
        dev, comp = _pick_device({"index": {"whisper_device": "vulkan"}},
                                 platform="win32", cpp_available=False)
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


class VulkanUrlTests(unittest.TestCase):
    """Источник Vulkan-сборки: официальный релиз → сборка сообщества (авто)."""

    def test_official_preferred(self):
        from hds.whisper_cpp import _vulkan_zip_url
        assets = [("whisper-vulkan-bin-x64.zip", "https://x/v.zip"),
                  ("whisper-bin-x64.zip", "https://x/cpu.zip")]
        self.assertEqual(_vulkan_zip_url(assets), "https://x/v.zip")

    def test_community_fallback_when_no_official(self):
        from hds.whisper_cpp import _UNOFFICIAL_VULKAN_URL, _vulkan_zip_url
        assets = [("whisper-bin-x64.zip", "https://x/cpu.zip"),
                  ("whisper-cublas-12.4.0-bin-x64.zip", "https://x/cu.zip")]
        self.assertEqual(_vulkan_zip_url(assets), _UNOFFICIAL_VULKAN_URL)


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


class RootSaveTests(unittest.TestCase):
    """Сохранение index.roots из UI: блок/inline-формы YAML, вставка, очистка."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-roots-")
        self.cfg = write_config(self.tmp)  # inline-форма: 'roots: []'
        self.addCleanup(lambda: os.path.exists(self.cfg) and os.remove(self.cfg))

    def test_save_block_form_preserves_neighbors(self):
        with open(self.cfg, "w", encoding="utf-8") as f:
            f.write("# тестовый конфиг\nindex:\n"
                    "  roots:\n    - 'D:\\old'\n"
                    "  exclude_dirs: [\"node_modules\"]\n")
        res = ui_server._set_roots(["D:\\new", "C:\\tmp"])
        self.assertTrue(res["ok"])
        cfg = load()
        self.assertEqual(cfg["index"]["roots"], ["D:\\new", "C:\\tmp"])
        self.assertIn("exclude_dirs", cfg["index"])  # соседний ключ не тронут
        self.assertIn("# тестовый конфиг", open(self.cfg, encoding="utf-8-sig").read())

    def test_save_inline_form(self):
        res = ui_server._set_roots(["D:\\x"])
        self.assertTrue(res["ok"])
        self.assertEqual(load()["index"]["roots"], ["D:\\x"])

    def test_save_empty_clears_and_warns(self):
        self.assertTrue(ui_server._set_roots(["D:\\x"])["ok"])
        res = ui_server._set_roots([])
        self.assertTrue(res["ok"])
        self.assertEqual(load()["index"]["roots"], [])
        self.assertIn("ВНИМАНИЕ", res["msg"])

    def test_relative_becomes_absolute(self):
        self.assertTrue(ui_server._set_roots(["docs"])["ok"])
        self.assertTrue(os.path.isabs(load()["index"]["roots"][0]))

    def test_not_a_list_refused(self):
        res = ui_server._set_roots("D:\\x")
        self.assertFalse(res["ok"])


class ReplaceFileTests(unittest.TestCase):
    """os.replace на Windows падает при конкурентном чтении config.yaml —
    replace_file должен ретраить."""

    def test_retries_then_succeeds(self):
        import types

        from hds import config as cfgmod

        d = tempfile.mkdtemp(prefix="hds-repl-")
        src, dst = os.path.join(d, "src"), os.path.join(d, "dst")
        with open(src, "w") as f:
            f.write("new")
        with open(dst, "w") as f:
            f.write("old")
        calls = {"n": 0}

        def flaky(a, b):
            calls["n"] += 1
            if calls["n"] < 3:
                raise PermissionError(32, "locked", b)
            return os.replace(a, b)

        # подменяем os только внутри модуля config (не глобально!)
        real_os = cfgmod.os
        cfgmod.os = types.SimpleNamespace(replace=flaky)
        try:
            cfgmod.replace_file(src, dst)
        finally:
            cfgmod.os = real_os
        self.assertEqual(calls["n"], 3)
        with open(dst) as f:
            self.assertEqual(f.read(), "new")

    def test_raises_after_attempts_exhausted(self):
        import types

        from hds import config as cfgmod

        d = tempfile.mkdtemp(prefix="hds-repl2-")
        src, dst = os.path.join(d, "src"), os.path.join(d, "dst")
        open(src, "w").close()
        open(dst, "w").close()

        def always_locked(a, b):
            raise PermissionError(32, "locked", b)

        real_os = cfgmod.os
        cfgmod.os = types.SimpleNamespace(replace=always_locked)
        try:
            with self.assertRaises(PermissionError):
                cfgmod.replace_file(src, dst, attempts=3, delay=0.01)
        finally:
            cfgmod.os = real_os


class UiVersionTests(unittest.TestCase):
    """/api/status отдаёт версию приложения — run_ui.ps1 сверяет её для
    авто-перезапуска устаревшего UI-сервера."""

    def test_version_exposed(self):
        from hds import __version__
        from hds import ui_server

        self.assertEqual(ui_server.HDS_VERSION, __version__)
        self.assertTrue(__version__)


class EmbModelTests(unittest.TestCase):
    """Сопоставление модели эмбеддингов (LM Studio может отдавать другой id)."""

    def test_match_exact(self):
        from hds.ui_server import _match_emb_model
        self.assertEqual(
            _match_emb_model(["qwen", "text-embedding-bge-m3"], "text-embedding-bge-m3"),
            ("text-embedding-bge-m3", None))

    def test_match_substring(self):
        from hds.ui_server import _match_emb_model
        exact, actual = _match_emb_model(["qwen", "lm-kit/bge-m3-gguf"],
                                         "text-embedding-bge-m3")
        self.assertIsNone(exact)
        self.assertEqual(actual, "lm-kit/bge-m3-gguf")

    def test_match_none(self):
        from hds.ui_server import _match_emb_model
        self.assertEqual(_match_emb_model(["qwen"], "text-embedding-bge-m3"),
                         (None, None))


class EmbModelSaveTests(unittest.TestCase):
    """«Применить имя модели»: замена embedding.model в config.yaml."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-embm-")
        self.cfg = write_config(self.tmp)
        self.addCleanup(lambda: os.path.exists(self.cfg) and os.remove(self.cfg))

    def test_adopt_replaces_model_keeps_neighbors(self):
        res = ui_server._set_embedding_model("text-embedding-bge-m3")
        self.assertTrue(res["ok"])
        self.assertEqual(load()["embedding"]["model"], "text-embedding-bge-m3")
        txt = open(self.cfg, encoding="utf-8-sig").read()
        self.assertIn("base_url", txt)   # соседние ключи на месте
        self.assertIn("batch_size", txt)

    def test_adopt_invalid_input(self):
        self.assertFalse(ui_server._set_embedding_model("   ")["ok"])
        self.assertFalse(ui_server._set_embedding_model(None)["ok"])


if __name__ == "__main__":
    unittest.main()
