"""Тесты общего llama-рантайма (hds.llama_runtime) и его интеграции с
менеджером hds.llama_server: пути рантайма по ОС, подсказки установщика,
манифест активной чат-модели, resolve_model('shared:<role>'), реестр
проектов, обзор для UI.

Сеть и реальные llama-инстансы не используются: каталог рантайма
подменяется временным через LLAMA_RUNTIME_DIR. Файл одинаково проходит и в
macOS-джобе CI (.github/workflows/ci.yml, test-macos).
"""
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import llama_runtime as lr  # noqa: E402
from hds import llama_server as ls  # noqa: E402


class TempRuntime(unittest.TestCase):
    """Общая база: временный LLAMA_RUNTIME_DIR на время теста."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="hds-runtime-")
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)
        patcher = mock.patch.dict(os.environ, {lr.RUNTIME_DIR_ENV: str(self.dir)})
        patcher.start()
        self.addCleanup(patcher.stop)


class RuntimeDirTests(unittest.TestCase):
    def test_env_override(self):
        with tempfile.TemporaryDirectory(prefix="hds-rt-") as td:
            with mock.patch.dict(os.environ, {lr.RUNTIME_DIR_ENV: td}):
                self.assertEqual(lr.runtime_dir(), Path(td))
                self.assertEqual(lr.bin_dir(), Path(td) / "bin")
                self.assertEqual(lr.models_dir(), Path(td) / "models")
                self.assertEqual(lr.models_dir("chat"), Path(td) / "models" / "chat")
                self.assertEqual(lr.current_file("chat"),
                                 Path(td) / "models" / "chat" / "current.json")
                self.assertEqual(lr.projects_file(), Path(td) / "projects.json")
                self.assertEqual(lr.version_file(), Path(td) / "version.json")

    def test_default_paths_per_os(self):
        """Без LLAMA_RUNTIME_DIR каталог рантайма зависит от ОС.

        Те же правила вычисляет установщик рантайма
        (ensure_llama_runtime.ps1 | .sh) — расхождение сломало бы поиск
        бинаря и моделей на macOS.
        """
        with mock.patch.dict(os.environ, {lr.RUNTIME_DIR_ENV: ""}):
            if sys.platform == "win32":
                local = str(Path.home() / "AppData" / "Local")
                with mock.patch.dict(os.environ, {"LOCALAPPDATA": local}):
                    self.assertEqual(lr.runtime_dir(),
                                     Path(local) / lr.DEFAULT_DIRNAME)
            elif sys.platform == "darwin":
                self.assertEqual(
                    lr.runtime_dir(),
                    Path.home() / "Library" / "Application Support" / lr.DEFAULT_DIRNAME)
            else:
                self.assertEqual(lr.runtime_dir(),
                                 Path.home() / ".local" / "share" / lr.DEFAULT_DIRNAME)

    def test_install_hint_per_os(self):
        """Подсказки в ошибках называют установщик текущей ОС (SYNC-COPY-пара)."""
        hint = lr.install_hint()
        if sys.platform == "win32":
            self.assertEqual(hint, "ensure_llama_runtime.ps1")
        else:
            self.assertEqual(hint, "ensure_llama_runtime.sh")


class FindBinaryTests(TempRuntime):
    def test_no_binary(self):
        self.assertEqual(lr.find_binary(), "")

    def test_binary_name_per_os(self):
        (self.dir / "bin").mkdir(parents=True, exist_ok=True)
        name = "llama-server.exe" if sys.platform == "win32" else "llama-server"
        exe = self.dir / "bin" / name
        exe.write_bytes(b"x")
        self.assertEqual(lr.find_binary(), str(exe))

    def test_symlinked_binary(self):
        """На macOS bin/llama-server — ссылка на бинарь Homebrew: find_binary
        обязан её разыменовать (иначе рантайм «не видит» установленный brew)."""
        if sys.platform == "win32":
            self.skipTest("на Windows в рантайме лежит обычный llama-server.exe")
        (self.dir / "bin").mkdir(parents=True, exist_ok=True)
        target = self.dir / "brew-llama-server"
        target.write_bytes(b"x")
        link = self.dir / "bin" / "llama-server"
        link.symlink_to(target)
        self.assertEqual(lr.find_binary(), str(link))

    def test_broken_symlink_ignored(self):
        if sys.platform == "win32":
            self.skipTest("символические ссылки в рантайме — случай macOS/Linux")
        (self.dir / "bin").mkdir(parents=True, exist_ok=True)
        (self.dir / "bin" / "llama-server").symlink_to(self.dir / "missing")
        self.assertEqual(lr.find_binary(), "")


class ManifestResolveTests(TempRuntime):
    def test_manifest_roundtrip_and_resolve(self):
        d = lr.models_dir("chat")
        d.mkdir(parents=True, exist_ok=True)
        (d / "a.gguf").write_bytes(b"a")
        (d / "b.gguf").write_bytes(b"b")

        # без манифеста при двух файлах shared:chat не разрешается
        with self.assertRaises(FileNotFoundError) as ctx:
            lr.resolve_model("shared:chat")
        self.assertIn("current.json", str(ctx.exception))

        lr.set_current_chat("a.gguf")
        self.assertEqual(lr.read_current("chat")["file"], "a.gguf")
        self.assertEqual(lr.resolve_model("shared:chat"), d / "a.gguf")
        self.assertEqual(lr.resolve_model("shared"), d / "a.gguf")  # роль по умолчанию

        # регистр имени в манифесте не важен (Windows/APFS регистронезависимы)
        lr.set_current_chat("A.GGUF")
        self.assertEqual(lr.resolve_model("shared:chat"), d / "a.gguf")

        # обычный путь возвращается как есть
        self.assertEqual(lr.resolve_model("/tmp/x.gguf"), Path("/tmp/x.gguf"))

    def test_single_model_is_active(self):
        d = lr.models_dir("embedding")
        d.mkdir(parents=True, exist_ok=True)
        (d / "only.gguf").write_bytes(b"x")
        self.assertEqual(lr.resolve_model("shared:embedding"), d / "only.gguf")

    def test_missing_file_error_mentions_installer(self):
        lr.models_dir("rerank").mkdir(parents=True, exist_ok=True)
        with self.assertRaises(FileNotFoundError) as ctx:
            lr.resolve_model("shared:rerank")
        self.assertIn(lr.install_hint(), str(ctx.exception))

    def test_set_current_chat_requires_file(self):
        lr.models_dir("chat").mkdir(parents=True, exist_ok=True)
        with self.assertRaises(FileNotFoundError):
            lr.set_current_chat("nope.gguf")
        with self.assertRaises(ValueError):
            lr.set_current_chat("   ")


class RegistryTests(TempRuntime):
    def test_register_and_switch(self):
        lr.register_project("proj-a", str(self.dir / "a"), ["-m", "pkg.a", "restart"])
        lr.register_project("proj-b", str(self.dir / "b"), ["-m", "pkg.b", "restart"])
        lr.register_project("proj-a", str(self.dir / "a"), ["-m", "pkg.a", "restart"])
        names = sorted(p["name"] for p in lr.list_projects())
        self.assertEqual(names, ["proj-a", "proj-b"])   # дубликата нет

        (lr.models_dir("chat") / "m.gguf").parent.mkdir(parents=True, exist_ok=True)
        (lr.models_dir("chat") / "m.gguf").write_bytes(b"m")
        info = lr.switch_chat_model("m.gguf", restart=False)
        self.assertTrue(info["ok"])
        self.assertEqual(info["file"], "m.gguf")
        self.assertEqual(lr.read_current("chat")["file"], "m.gguf")
        self.assertNotIn("applied", info)               # рестартов не было

        bad = lr.switch_chat_model("ghost.gguf", restart=False)
        self.assertFalse(bad["ok"])
        self.assertIn("ghost.gguf", bad["msg"])

    def test_known_preset_without_download(self):
        lr.models_dir("chat").mkdir(parents=True, exist_ok=True)
        preset = sorted(lr.CHAT_PRESETS)[0]
        info = lr.switch_chat_model(preset, restart=False, download=False)
        self.assertFalse(info["ok"])
        self.assertTrue(info.get("need_download"))


class OverviewTests(TempRuntime):
    def test_overview_for_ui(self):
        d = lr.models_dir("chat")
        d.mkdir(parents=True, exist_ok=True)
        (d / "m.gguf").write_bytes(b"m")
        lr.set_current_chat("m.gguf")
        ov = lr.chat_models_overview()
        self.assertEqual(ov["current"], "m.gguf")
        self.assertTrue(any(a["file"] == "m.gguf" for a in ov["available"]))
        self.assertEqual(len(ov["presets"]), len(lr.CHAT_PRESETS))
        self.assertFalse(ov["binary_ok"])               # рантайм пуст
        self.assertEqual(ov["runtime"], str(self.dir))


class ServerIntegrationTests(TempRuntime):
    def test_shared_model_resolves_from_manifest(self):
        d = lr.models_dir("embedding")
        d.mkdir(parents=True, exist_ok=True)
        (d / "bge.gguf").write_bytes(b"x")
        cfg = {"llm_server": {"embedding": {"model": "shared:embedding"}}}
        self.assertEqual(ls._abs_model(cfg, "embedding"),
                         os.path.normpath(str(d / "bge.gguf")))

    def test_no_runtime_binary_gives_installer_hint(self):
        """Пустой рантайм + пустой PATH: build_command выдаёт понятную ошибку
        с установщиком для текущей ОС (на macOS — bash-версия)."""
        cfg = {"llm_server": {"bin": "", "chat": {"model": "shared:chat"}}}
        with mock.patch.object(ls.shutil, "which", return_value=None):
            with self.assertRaises(RuntimeError) as ctx:
                ls.build_command(cfg, "chat")
        msg = str(ctx.exception)
        self.assertIn("ensure_llama_runtime", msg)
        self.assertIn(lr.install_hint(), msg)

    def test_binary_from_runtime(self):
        (self.dir / "bin").mkdir(parents=True, exist_ok=True)
        name = "llama-server.exe" if sys.platform == "win32" else "llama-server"
        (self.dir / "bin" / name).write_bytes(b"x")
        cfg = {"llm_server": {"bin": ""}}
        self.assertEqual(ls.find_binary(cfg), lr.find_binary())
        self.assertTrue(ls.find_binary(cfg))

    def test_explicit_bin_wins(self):
        cfg = {"llm_server": {"bin": "C:/custom/llama-server"}}
        self.assertEqual(ls.find_binary(cfg), "C:/custom/llama-server")


if __name__ == "__main__":
    unittest.main(verbosity=2)