"""РўРµСЃС‚С‹ РґРёР°РіРЅРѕСЃС‚РёРєРё: РєРѕРЅС‚РµРєСЃС‚ embedding-РёРЅСЃС‚Р°РЅСЃР° llama-server (/props)."""
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import diag  # noqa: E402
from hds import llama_server as ls  # noqa: E402
from hds import lemmatizer as _lemmatizer  # noqa: E402


class EmbeddingContextDiagTests(unittest.TestCase):
    """РџСЂРё РєРѕРЅС‚РµРєСЃС‚Рµ РјРµРЅСЊС€Рµ EMB_CONTEXT РґР»РёРЅРЅС‹Рµ С‡Р°РЅРєРё С‚РµСЂСЏСЋС‚ С…РІРѕСЃС‚, РїРѕСЌС‚РѕРјСѓ
    `check` (Рё РєРЅРѕРїРєР° В«РџСЂРѕРІРµСЂРёС‚СЊВ» РІ UI) РѕР±СЏР·Р°РЅ РїРѕРєР°Р·Р°С‚СЊ Р·Р°РЅРёР¶РµРЅРЅС‹Р№ РєРѕРЅС‚РµРєСЃС‚.
    РћР±РѕСЃРЅРѕРІР°РЅРёРµ вЂ” Р·Р°РјРµСЂ: РїСЂРё ctx=512 РѕРєРѕР»Рѕ 45 % С‡Р°РЅРєРѕРІ РїСЂРѕРµРєС‚Р° РїРѕС‚РµСЂСЏР»Рё Р±С‹
    С…РІРѕСЃС‚ (РјРµРґРёР°РЅР° 484 С‚РѕРєРµРЅР°, p90 829, РјР°РєСЃРёРјСѓРј 2114)."""

    def _props(self, ctx):
        return {"total_slots": 1,
                "default_generation_settings": {"n_ctx": ctx}}

    def _patch(self, ctx):
        """probe СЂРѕР»Рё embedding: Р¶РёРІРѕР№ llama СЃ РєРѕРЅС‚РµРєСЃС‚РѕРј ctx."""
        det = {"state": ls.STATE_LLAMA, "total_slots": 1,
               "props": self._props(ctx)}
        return mock.patch.object(ls, "probe", lambda cfg, role, **kw: det)

    def test_context_reported(self):
        self.assertEqual(ls.props_context(self._props(8192)), 8192)

    def test_small_context_warns(self):
        import tempfile
        from helpers import write_config  # noqa: I100

        tmp = tempfile.mkdtemp(prefix="hds-diag-ctx-")
        cfg_path = write_config(tmp)
        self.addCleanup(lambda: os.path.exists(cfg_path)
                        and os.remove(cfg_path))
        from hds.config import EMB_CONTEXT, load
        self.assertLess(512, EMB_CONTEXT)
        with self._patch(512):
            checks = diag.run_checks(load())
        embctx = [c for c in checks if c["id"] == "embctx"]
        self.assertTrue(embctx, "Р·Р°РЅРёР¶РµРЅРЅС‹Р№ РєРѕРЅС‚РµРєСЃС‚ РѕР±СЏР·Р°РЅ РїРѕРїР°РґР°С‚СЊ РІ check")
        self.assertEqual(embctx[0]["status"], "warn")
        self.assertIn("512", embctx[0]["title"])

    def test_correct_context_no_warn(self):
        import tempfile
        from helpers import write_config  # noqa: I100

        tmp = tempfile.mkdtemp(prefix="hds-diag-ctx2-")
        cfg_path = write_config(tmp)
        self.addCleanup(lambda: os.path.exists(cfg_path)
                        and os.remove(cfg_path))
        from hds.config import load
        with self._patch(8192):
            checks = diag.run_checks(load())
        self.assertFalse([c for c in checks if c["id"] == "embctx"])


class FtsNormDiagTests(unittest.TestCase):
    """РџСЂРѕРІРµСЂРєР° fts-norm СЃРѕРґРµСЂР¶Р°С‚РµР»СЊРЅР°СЏ (СЃСЌРјРїР» chunks vs chunks_fts), Р° РЅРµ РїРѕ
    С„Р»Р°РіСѓ meta: РѕР±С‹С‡РЅР°СЏ РёРЅРґРµРєСЃР°С†РёСЏ РІСЃРµРіРґР° РїРёС€РµС‚ Р»РµРјРјР°С‚РёР·РёСЂРѕРІР°РЅРЅС‹Р№ FTS
    (db.add_chunk -> lemmatizer.normalize), С„Р»Р°Рі СЃС‚Р°РІРёР»Р° С‚РѕР»СЊРєРѕ РєРѕРјР°РЅРґР°
    reindex-fts вЂ” РЅР° СЃРІРµР¶РµР№ СѓСЃС‚Р°РЅРѕРІРєРµ В«РёРЅРґРµРєСЃР°С†РёСЏ СЃ РЅСѓР»СЏВ» check Р»РѕР¶РЅРѕ
    С‚СЂРµР±РѕРІР°Р» reindex-fts."""

    def _checks(self):
        import tempfile
        from helpers import write_config  # noqa: I100

        tmp = tempfile.mkdtemp(prefix="hds-diag-fts-")
        write_config(tmp)
        from hds import db as dbmod  # noqa: E402
        from hds.config import db_abs_path, load  # noqa: E402

        conn = dbmod.connect(db_abs_path(load()), 8)
        fid = dbmod.upsert_file(conn, "a.txt", ".txt", "text", 10, 1.0, "h")
        dbmod.add_chunk(conn, fid, 0, None, None, None,
                        "РР·РјРµРЅРµРЅРёРµ РЅР°СЃС‚СЂРѕРµРє РїСЂРёР»РѕР¶РµРЅРёСЏ РѕРїРёСЃР°РЅРѕ РІ РґРѕРєСѓРјРµРЅС‚Р°С†РёРё")
        dbmod.add_chunk(conn, fid, 1, None, None, None,
                        "РћС‚С‡С‘С‚ РїРѕ СЃРєР»Р°РґСѓ: Р»РѕРіРёСЃС‚РёРєР° Рё РјР°СЂС€СЂСѓС‚С‹ РґРѕСЃС‚Р°РІРєРё")
        conn.close()
        with mock.patch.object(ls, "probe",
                               lambda cfg, role, **kw: {"state": ls.STATE_LLAMA, "total_slots": 1, "props": {"total_slots": 1, "default_generation_settings": {"n_ctx": 8192}}}):
            checks = diag.run_checks(load())
        return [c for c in checks if c["id"] == "fts-norm"]

    @unittest.skipUnless(_lemmatizer.available(), "pymorphy3 РЅРµ СѓСЃС‚Р°РЅРѕРІР»РµРЅ")
    def test_fresh_index_no_warn(self):
        """Р Р•Р“Р Р•РЎРЎРРЇ: СЃРІРµР¶РёР№ РёРЅРґРµРєСЃ Р»РµРјРјР°С‚РёР·РёСЂРѕРІР°РЅ вЂ” РїСЂРµРґСѓРїСЂРµР¶РґРµРЅРёСЏ Р±С‹С‚СЊ РЅРµ РґРѕР»Р¶РЅРѕ."""
        self.assertEqual(self._checks(), [])

    @unittest.skipUnless(_lemmatizer.available(), "pymorphy3 РЅРµ СѓСЃС‚Р°РЅРѕРІР»РµРЅ")
    def test_raw_fts_warns(self):
        """РЎС‚Р°СЂС‹Рµ РґР°РЅРЅС‹Рµ (FTS Р±РµР· Р»РµРјРјР°С‚РёР·Р°С†РёРё) РґРѕР»Р¶РЅС‹ РґР°РІР°С‚СЊ РїСЂРµРґСѓРїСЂРµР¶РґРµРЅРёРµ."""
        import tempfile
        from helpers import write_config  # noqa: I100

        tmp = tempfile.mkdtemp(prefix="hds-diag-ftsr-")
        write_config(tmp)
        from hds import db as dbmod  # noqa: E402
        from hds.config import db_abs_path, load  # noqa: E402

        conn = dbmod.connect(db_abs_path(load()), 8)
        fid = dbmod.upsert_file(conn, "b.txt", ".txt", "text", 10, 1.0, "h")
        dbmod.add_chunk(conn, fid, 0, None, None, None,
                        "РР·РјРµРЅРµРЅРёРµ РЅР°СЃС‚СЂРѕРµРє РїСЂРёР»РѕР¶РµРЅРёСЏ")
        # РёРјРёС‚РёСЂСѓРµРј С‡Р°РЅРєРё, РїСЂРѕРёРЅРґРµРєСЃРёСЂРѕРІР°РЅРЅС‹Рµ РґРѕ РІРєР»СЋС‡РµРЅРёСЏ Р»РµРјРјР°С‚РёР·Р°С†РёРё:
        # РІ chunks_fts вЂ” РёСЃС…РѕРґРЅС‹Рµ СЃР»РѕРІРѕС„РѕСЂРјС‹ Р±РµР· pymorphy3
        conn.execute("UPDATE chunks_fts SET text=? WHERE rowid=?",
                     ("РёР·РјРµРЅРµРЅРёРµ РЅР°СЃС‚СЂРѕР№РєРё РїСЂРёР»РѕР¶РµРЅРёСЏ", 1))
        conn.commit()
        conn.close()
        with mock.patch.object(ls, "probe",
                               lambda cfg, role, **kw: {"state": ls.STATE_LLAMA, "total_slots": 1, "props": {"total_slots": 1, "default_generation_settings": {"n_ctx": 8192}}}):
            checks = diag.run_checks(load())
        fts = [c for c in checks if c["id"] == "fts-norm"]
        self.assertTrue(fts, "РЅРµСЃРѕРІРїР°РґРµРЅРёРµ СЃСЌРјРїР»РѕРІ РґРѕР»Р¶РЅРѕ РґР°РІР°С‚СЊ РїСЂРµРґСѓРїСЂРµР¶РґРµРЅРёРµ")
        self.assertEqual(fts[0]["status"], "warn")


if __name__ == "__main__":
    unittest.main()
