"""Real offline CPU measurement; run with python -m unittest discover -s eval/tests -p test_harness.py."""

import hashlib
import json
import math
import os
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace

import torch
from tokenizers import Tokenizer
from tokenizers.models import WordLevel
from tokenizers.pre_tokenizers import Whitespace
from transformers import GPT2Config, GPT2LMHeadModel, PreTrainedTokenizerFast

from proof_eval.contract import ContractError
from proof_eval.harness import SCORED_SPLITS, measure


class HarnessTest(unittest.TestCase):
    def setUp(self):
        torch.set_num_threads(1)
        torch.manual_seed(7)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.artifact = self.root / "artifact"
        self.artifact.mkdir()
        self.store = self.root / "holdout"
        self.store.mkdir()
        previous = os.environ.get("PROOF_HOLDOUT_STORE")
        os.environ["PROOF_HOLDOUT_STORE"] = str(self.store)
        def restore_store():
            if previous is None:
                os.environ.pop("PROOF_HOLDOUT_STORE", None)
            else:
                os.environ["PROOF_HOLDOUT_STORE"] = previous
        self.addCleanup(restore_store)
        raw = Tokenizer(WordLevel({"[UNK]": 0, "one": 1, "two": 2, "three": 3}, unk_token="[UNK]"))
        raw.pre_tokenizer = Whitespace()
        self.tok = PreTrainedTokenizerFast(tokenizer_object=raw, unk_token="[UNK]")
        self.tok.save_pretrained(self.artifact)
        self.model = GPT2LMHeadModel(GPT2Config(
            vocab_size=4, n_positions=1024, n_embd=8, n_layer=1, n_head=1,
            bos_token_id=0, eos_token_id=0,
        )).eval()
        self.model.save_pretrained(self.artifact, safe_serialization=True)
        self.text = "one two three one two"
        self.digest = hashlib.sha256(self.text.encode()).hexdigest()
        (self.store / self.digest).write_text(self.text)
        self.request = SimpleNamespace(
            family="throughput", model_ref="invalid-judge/model", proxy_model="invalid-proxy/model",
            holdout=[{"id": i, "split": split, "content_sha256": self.digest}
                     for i, split in enumerate(SCORED_SPLITS)],
        )

    def score(self):
        return measure(self.request, str(self.artifact))

    def test_real_nll_matches_independent_forward(self):
        # Independent forward on original in-memory weights, not the reloaded harness model.
        with torch.no_grad():
            logits = self.model(torch.tensor([[1, 2, 3, 1, 2]])).logits[:, :-1, :]
            expected = torch.nn.functional.cross_entropy(
                logits.reshape(-1, 4), torch.tensor([2, 3, 1, 2]),
            ).item()
        result = self.score()
        self.assertAlmostEqual(result["holdout_nll"], expected, places=6)
        self.assertEqual(set(result["split_nll"]), set(SCORED_SPLITS))
        for nll in result["split_nll"].values():
            self.assertAlmostEqual(nll, expected, places=6)
        self.assertTrue(math.isfinite(result["tokens_per_sec"]))
        self.assertGreater(result["tokens_per_sec"], 0)
        self.assertNotIn("clean", result)
        self.assertNotIn("reproduced", result)

    def test_no_proxy_fallback(self):
        with self.assertRaisesRegex(ContractError, "local data-only"):
            measure(self.request, None)

    def test_corrupt_holdout(self):
        (self.store / self.digest).write_text("tampered")
        with self.assertRaisesRegex(ContractError, "mismatch"):
            self.score()

    def test_digest_is_exact_lowercase_hex(self):
        for digest in (self.digest.upper(), "z" * 64, " " + self.digest, "../" + "a" * 61):
            with self.subTest(digest=digest):
                self.request.holdout[0]["content_sha256"] = digest
                with self.assertRaisesRegex(ContractError, "malformed"):
                    self.score()

    def test_missing_tokenizer(self):
        (self.artifact / "tokenizer.json").unlink()
        with self.assertRaisesRegex(ContractError, "missing local artifact tokenizer"):
            self.score()

    def test_custom_auto_map(self):
        for name in ("config.json", "tokenizer_config.json"):
            path = self.artifact / name
            original = path.read_text()
            config = json.loads(original)
            config["auto_map"] = {"AutoModelForCausalLM": "evil.Replacement"}
            path.write_text(json.dumps(config))
            with self.assertRaisesRegex(ContractError, "auto_map"):
                self.score()
            path.write_text(original)

    def test_pickle_only_model(self):
        (self.artifact / "model.safetensors").unlink()
        torch.save(self.model.state_dict(), self.artifact / "pytorch_model.bin")
        with self.assertRaisesRegex(ContractError, "unsupported artifact file"):
            self.score()

    def test_missing_split(self):
        self.request.holdout.pop()
        with self.assertRaisesRegex(ContractError, "missing required holdout splits"):
            self.score()

    def test_unknown_or_absent_split(self):
        for split in ("unknown", None):
            self.request.holdout[0]["split"] = split
            with self.assertRaisesRegex(ContractError, "unknown or missing"):
                self.score()

    def test_symlink_artifact_entry(self):
        path = self.artifact / "tokenizer.json"
        external = self.root / "tokenizer.json"
        path.rename(external)
        path.symlink_to(external)
        with self.assertRaisesRegex(ContractError, "links"):
            self.score()

    def test_index_path_escape(self):
        (self.artifact / "model.safetensors.index.json").write_text(json.dumps({
            "weight_map": {"weight": "../outside.safetensors"},
        }))
        with self.assertRaisesRegex(ContractError, "shard path"):
            self.score()

    def test_config_path_escape(self):
        path = self.artifact / "tokenizer_config.json"
        config = json.loads(path.read_text())
        config["tokenizer_file"] = "/tmp/other.json"
        path.write_text(json.dumps(config))
        with self.assertRaisesRegex(ContractError, "nonlocal artifact reference"):
            self.score()

    def test_custom_code_file(self):
        (self.artifact / "modeling_custom.py").write_text("raise RuntimeError('must not execute')")
        with self.assertRaisesRegex(ContractError, "unsupported artifact file"):
            self.score()

    def test_nonfinite_loss(self):
        with torch.no_grad():
            self.model.transformer.wte.weight.fill_(float("nan"))
        self.model.save_pretrained(self.artifact, safe_serialization=True)
        with self.assertRaisesRegex(ContractError, "non-finite holdout loss"):
            self.score()


if __name__ == "__main__":
    unittest.main()
