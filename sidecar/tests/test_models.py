"""Пути моделей: закрепление весов GigaAM и HF-кэш рядом с поставкой."""
from __future__ import annotations

import os
import tempfile
import unittest
from unittest import mock

from hds_transcribe import models
from hds_transcribe.errors import JobError
from hds_transcribe.params import AsrParams


class GigaamSourceTest(unittest.TestCase):
    def test_empty_revision_uses_variant(self):
        self.assertEqual(models.resolve_gigaam_source(AsrParams(variant="e2e_rnnt")), "e2e_rnnt")

    def test_revision_file_is_used_as_is(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = os.path.join(tmp, "pinned.ckpt")
            with open(ckpt, "wb") as f:
                f.write(b"x")
            asr = AsrParams(variant="e2e_rnnt", revision=ckpt)
            self.assertEqual(models.resolve_gigaam_source(asr), ckpt)

    def test_revision_directory_looks_for_variant_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = os.path.join(tmp, "v3_e2e_rnnt.ckpt")
            with open(ckpt, "wb") as f:
                f.write(b"x")
            asr = AsrParams(variant="e2e_rnnt", revision=tmp)
            self.assertEqual(models.resolve_gigaam_source(asr), ckpt)

    def test_revision_directory_without_weights_raises_model_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(JobError) as ctx:
                models.resolve_gigaam_source(AsrParams(variant="e2e_rnnt", revision=tmp))
            self.assertEqual(ctx.exception.code, "model_missing")

    def test_non_path_revision_is_bad_request(self):
        with self.assertRaises(JobError) as ctx:
            models.resolve_gigaam_source(AsrParams(variant="e2e_rnnt", revision="deadbeef"))
        self.assertEqual(ctx.exception.code, "bad_request")


class HfHomeTest(unittest.TestCase):
    def test_hf_home_from_models_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {"HDS_MODELS_DIR": tmp}
            with mock.patch.dict(os.environ, env, clear=False):
                os.environ.pop("HF_HOME", None)
                self.assertEqual(models.hf_home(), os.path.join(tmp, "hf-home"))

    def test_explicit_hf_home_wins(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {"HDS_MODELS_DIR": os.path.join(tmp, "models"), "HF_HOME": os.path.join(tmp, "hf")}
            with mock.patch.dict(os.environ, env, clear=False):
                self.assertEqual(models.hf_home(), env["HF_HOME"])


if __name__ == "__main__":
    unittest.main()
