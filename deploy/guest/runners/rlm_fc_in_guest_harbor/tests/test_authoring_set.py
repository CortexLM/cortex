#!/usr/bin/env python3
"""authoring_set.py: the RLM authors its whole set, or it refuses.

What is pinned here, in the order the failure modes matter:

* a complete set — every one of the five parts present and shaped the way the
  guest and the install hold them (the same `proof-topic-authoring` checks);
* **rules-only is not authorship**: the entrypoint writes `authoring.json` and
  never `rules.json`, and a missing part is a refusal naming the part;
* the operator's bundle is not the source of truth: the vector is framed by
  the RLM, the migration sits in the topic's namespace, the pin policy
  **restates** the document, and a policy that would diverge is not written;
* re-authoring retains what it is not changing, and a previous set for another
  topic is refused;
* a declared rule with no signed policy, and a marker policy with no declared
  rule, are both refusals — never a silent drop or an invented check.

No guest agent, no VM, no Harbor: `main()` is driven directly with the
environment contract the guest exports for `ProposeRules`.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import authoring_set  # noqa: E402

TOPIC_ID = "fixture-topic-v0"
MARKERS = "no_short_circuit:skip_eval|skip_verifier;no_answer_table:answer_key"
ATTESTED = "miner_pays_provider,same_seed"


def topic_document(**overrides: object) -> dict:
    doc = {
        "schema_version": 1,
        "id": TOPIC_ID,
        "statement": "score the pinned pack with the pinned runner",
        "status": "draft",
        "constraints": {
            "firecracker_required": True,
            "model_pin": "vendor/model",
            "params": {
                "baseline_runner": "rlm_fc_in_guest_harbor",
                "experiment_pack_digest": "sha256:" + "ab" * 32,
                "tasks_dir": "tasks",
            },
        },
        "metric": {
            "family": "custom",
            "primary": "success_rate",
            "direction": "max",
            "epsilon_rel": 0.05,
            "custom_id": "fixture_metric",
        },
        "checklist": [
            {"id": "no_short_circuit", "text": "the evaluator and the metric path are untouched"},
            {"id": "no_answer_table", "text": "no hardcoded answers for the scored set"},
            {"id": "miner_pays_provider", "text": "the miner pays for its own provider calls"},
        ],
        "epsilon_nll": 0.02,
        "epsilon_topic_max_regress": 0.05,
        "flops_budget": 2_000_000_000_000_000_000,
        "holdout_size": 120,
        "eval_executor": {"max_proof_deadline_s": 3600},
    }
    doc.update(overrides)
    return doc


class Harness:
    """One `main()` run: a temp root, the guest's env, the parsed answer."""

    def __init__(self, doc: dict, previous: dict | None = None, markers=MARKERS, attested=ATTESTED):
        self.root = Path(tempfile.mkdtemp(prefix="authoring-set-"))
        self.topic = self.root / "topic.json"
        self.topic.write_text(json.dumps(doc), encoding="utf-8")
        self.output = self.root / "output"
        self.work = self.root / "work"
        self.output.mkdir()
        self.work.mkdir()
        env = {
            "PROOF_JOB": "propose_rules",
            "PROOF_TOPIC_ID": doc.get("id", ""),
            "PROOF_CUSTOM_ID": (doc.get("metric") or {}).get("custom_id", ""),
            "PROOF_TOPIC_FILE": str(self.topic),
            "PROOF_OUTPUT_DIR": str(self.output),
            "PROOF_WORK_DIR": str(self.work),
            "PROOF_CURRENT_RULES_VERSION": "",
        }
        if previous is not None:
            path = self.work / authoring_set.CURRENT_AUTHORING_FILE
            path.write_text(json.dumps(previous), encoding="utf-8")
            env["PROOF_CURRENT_AUTHORING_FILE"] = str(path)
        else:
            env["PROOF_CURRENT_AUTHORING_FILE"] = ""
        if markers is not None:
            env[authoring_set.PARAM_MARKER_RULES] = markers
        if attested is not None:
            env[authoring_set.PARAM_ATTESTED_RULES] = attested
        self.env = env

    def run(self) -> int:
        saved = {k: os.environ.get(k) for k in self.env}
        os.environ.update(self.env)
        try:
            return authoring_set.main([])
        finally:
            for k, v in saved.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v

    def set_path(self) -> Path:
        return self.output / authoring_set.AUTHORING_FILE

    def authored(self) -> dict:
        return json.loads(self.set_path().read_text(encoding="utf-8"))

    def cleanup(self) -> None:
        import shutil

        shutil.rmtree(self.root, ignore_errors=True)


class CompleteSet(unittest.TestCase):
    def test_a_complete_set_carries_every_part_the_install_applies(self):
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        set_ = h.authored()
        self.assertEqual(set_["schema_version"], 1)
        self.assertEqual(set_["topic_id"], TOPIC_ID)
        # The five parts, each present and non-empty: what the install reads.
        self.assertEqual(
            [r["id"] for r in set_["rules"]],
            ["no_short_circuit", "no_answer_table", "miner_pays_provider"],
        )
        self.assertTrue(set_["migrations"], "a complete set carries a migration")
        self.assertTrue(set_["apis"], "a complete set carries a route")
        self.assertTrue(set_["submission_format"])
        self.assertIn("pin_policy", set_)
        # `deny_unknown_fields`: the parts this build reads are exactly these.
        self.assertEqual(
            sorted(set_),
            [
                "apis",
                "migrations",
                "pin_policy",
                "rules",
                "schema_version",
                "submission_format",
                "topic_id",
            ],
        )

    def test_the_entrypoint_writes_authoring_json_and_never_rules_json(self):
        """A rules-only answer is a fragment, and a fragment is not this job."""
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        self.assertTrue(h.set_path().is_file())
        self.assertFalse(
            (h.output / "rules.json").exists(),
            "writing rules.json would answer with a fragment the host refuses to open on",
        )

    def test_the_rules_are_the_rlms_framing_with_the_declaration_quoted(self):
        """The operator's sentence is the declaration, not the whole answer."""
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        rules = {r["id"]: r["text"] for r in h.authored()["rules"]}
        marker_text = rules["no_short_circuit"]
        self.assertIn("2 off-limits markers", marker_text)
        self.assertIn("rlm: artefact scan", marker_text)
        self.assertIn("the topic's declaration:", marker_text)
        attested_text = rules["miner_pays_provider"]
        self.assertIn("rlm: host/topic attestation", attested_text)
        self.assertIn("the topic's declaration:", attested_text)
        # Not a verbatim echo: the RLM says what it will prove, and how.
        for rid, text in rules.items():
            declared = next(r for r in topic_document()["checklist"] if r["id"] == rid)
            self.assertNotEqual(
                text.strip(),
                declared["text"].strip(),
                f"{rid} is the operator's sentence alone, which is a restatement not authorship",
            )

    def test_the_migration_is_inside_the_topics_own_namespace(self):
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        migrations = h.authored()["migrations"]
        self.assertEqual(len(migrations), 1)
        self.assertTrue(migrations[0]["name"], "a migration carries a name")
        prefix = TOPIC_ID.replace("-", "_")
        self.assertIn(f"{prefix}_", migrations[0]["sql"])
        for denied in ("proof_", "pg_catalog", "_sqlx_migrations"):
            self.assertNotIn(denied, migrations[0]["sql"].lower())

    def test_the_route_is_relative_and_outside_the_admin_namespace(self):
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        for route in h.authored()["apis"]:
            self.assertFalse(route["path"].startswith("/"), "a topic route is relative")
            self.assertNotEqual(route["path"].split("/")[0], "v1")
            self.assertIn(route["method"], ("GET", "POST", "PUT", "PATCH", "DELETE", "*"))

    def test_the_pin_policy_restates_the_document_and_invents_nothing(self):
        """Scoring reads the document, so the policy restates it exactly."""
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        policy = h.authored()["pin_policy"]
        doc = topic_document()
        self.assertEqual(policy["epsilon_nll_min"], doc["epsilon_nll"])
        self.assertEqual(
            policy["epsilon_topic_max_regress_min"], doc["epsilon_topic_max_regress"]
        )
        self.assertEqual(policy["epsilon_throughput_rel_min"], doc["metric"]["epsilon_rel"])
        self.assertEqual(policy["max_proof_deadline_s"], 3600)
        self.assertEqual(policy["flops_budget_max"], doc["flops_budget"])
        self.assertEqual(policy["holdout_size"], doc["holdout_size"])
        # The two pin equalities the VM cannot read are absent, never invented.
        self.assertNotIn("eval_image_digest", policy)
        self.assertNotIn("gpu_class", policy)

    def test_the_submission_format_is_the_hosts_real_intake_shape(self):
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        fmt = h.authored()["submission_format"]
        self.assertEqual(fmt["max_bytes"], authoring_set.MAX_ARTIFACT_BYTES)
        self.assertEqual(fmt["signature_domain"], authoring_set.SUBMIT_DOMAIN)
        self.assertIn("submit_nonce", json.dumps(fmt))

    def test_a_knob_the_document_does_not_declare_is_not_authored(self):
        """A policy restates; an absent knob is absent, never guessed."""
        doc = topic_document()
        doc.pop("epsilon_topic_max_regress")
        doc["eval_executor"] = {}
        h = Harness(doc)
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        policy = h.authored()["pin_policy"]
        self.assertNotIn("epsilon_topic_max_regress_min", policy)
        self.assertNotIn("max_proof_deadline_s", policy)
        self.assertIn("epsilon_nll_min", policy)


class Refusals(unittest.TestCase):
    def test_a_declared_rule_with_no_signed_policy_is_refused_by_name(self):
        """Neither an invented check nor a silent drop: the topic must say."""
        h = Harness(topic_document(), markers="no_short_circuit:skip_eval", attested="")
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit) as ctx:
            h.run()
        self.assertEqual(ctx.exception.code, 2)
        self.assertFalse(h.set_path().exists(), "a refused run writes no set")

    def test_no_rule_policy_at_all_is_refused(self):
        h = Harness(topic_document(), markers=None, attested=None)
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()
        self.assertFalse(h.set_path().exists())

    def test_a_marker_policy_for_an_undeclared_rule_is_refused(self):
        h = Harness(topic_document(), markers=MARKERS + ";never_declared:oops", attested=ATTESTED)
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()
        self.assertFalse(h.set_path().exists())

    def test_a_rule_named_as_both_marker_and_attested_is_refused(self):
        h = Harness(topic_document(), markers=MARKERS, attested=ATTESTED + ",no_short_circuit")
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()

    def test_an_empty_checklist_is_refused(self):
        h = Harness(topic_document(checklist=[]))
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()
        self.assertFalse(h.set_path().exists())

    def test_the_wrong_job_is_refused(self):
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        h.env["PROOF_JOB"] = "evaluate"
        with self.assertRaises(SystemExit):
            h.run()

    def test_a_missing_topic_file_is_refused(self):
        h = Harness(topic_document())
        self.addCleanup(h.cleanup)
        h.env["PROOF_TOPIC_FILE"] = str(h.root / "nope.json")
        with self.assertRaises(SystemExit):
            h.run()

    def test_a_previous_set_for_another_topic_is_refused(self):
        prior = {"schema_version": 1, "topic_id": "someone-elses-topic"}
        h = Harness(topic_document(), previous=prior)
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()
        self.assertFalse(h.set_path().exists())

    def test_a_migration_outside_the_namespace_is_refused(self):
        """The scope check refuses what the install's deny-list would."""
        with self.assertRaises(SystemExit):
            authoring_set.check_migration_scope("DROP TABLE proof_rule_version", TOPIC_ID)
        with self.assertRaises(SystemExit):
            authoring_set.check_migration_scope("SELECT * FROM other_topic_rows", TOPIC_ID)
        with self.assertRaises(SystemExit):
            authoring_set.check_migration_scope("SELECT * FROM pg_catalog.pg_class", TOPIC_ID)
        # The topic's own objects pass, in both accepted spellings. An index
        # name is an object too: the guard scopes it the same way.
        prefix = TOPIC_ID.replace("-", "_")
        authoring_set.check_migration_scope(
            f"CREATE TABLE {prefix}_scratch (id TEXT)", TOPIC_ID
        )
        authoring_set.check_migration_scope(
            f"CREATE INDEX {prefix}_scratch_idx ON {prefix}_scratch (id)", TOPIC_ID
        )
        # The literal hyphen id is one legal **quoted** identifier, not three
        # unscoped words (the guard's own `tokens` keeps a quoted run whole).
        authoring_set.check_migration_scope(
            f'CREATE TABLE "{TOPIC_ID}_scratch" (id TEXT)', TOPIC_ID
        )
        with self.assertRaises(SystemExit):
            authoring_set.check_migration_scope(
                f"CREATE INDEX unscoped_idx ON {prefix}_scratch (id)", TOPIC_ID
            )

    def test_a_malformed_route_is_refused(self):
        with self.assertRaises(SystemExit):
            authoring_set.check_api({"path": "/absolute", "method": "GET"})
        with self.assertRaises(SystemExit):
            authoring_set.check_api({"path": "v1/admin/exec", "method": "GET"})
        with self.assertRaises(SystemExit):
            authoring_set.check_api({"path": "status", "method": "TRACE"})
        with self.assertRaises(SystemExit):
            authoring_set.check_api({"path": "a/../b", "method": "GET"})
        # A method is normalised the way the install reads it.
        self.assertEqual(
            authoring_set.check_api({"path": "status", "method": "get"})["method"], "GET"
        )

    def test_a_malformed_pin_policy_is_refused(self):
        with self.assertRaises(SystemExit):
            authoring_set.check_pin_policy({"epsilon_nll_min": 0})
        with self.assertRaises(SystemExit):
            authoring_set.check_pin_policy({"epsilon_nll_min": 1.5})
        with self.assertRaises(SystemExit):
            authoring_set.check_pin_policy({"max_proof_deadline_s": 0})
        with self.assertRaises(SystemExit):
            authoring_set.check_pin_policy({"not_a_knob": 1})
        # `{}` is an answer: the RLM saying this topic tightens nothing.
        self.assertEqual(authoring_set.check_pin_policy({}), {})


class ReAuthoring(unittest.TestCase):
    def previous_set(self) -> dict:
        return {
            "schema_version": 1,
            "topic_id": TOPIC_ID,
            "rules": [{"id": "stale_rule", "text": "a rule the re-signed topic dropped"}],
            "migrations": [
                {
                    "name": "0001_scratch",
                    "sql": f"CREATE TABLE {TOPIC_ID.replace('-', '_')}_scratch (id TEXT)",
                },
                {"name": "0002_kept", "sql": f"CREATE TABLE {TOPIC_ID.replace('-', '_')}_kept (id TEXT)"},
            ],
            "apis": [
                {"path": "status", "method": "GET", "summary": "the old summary"},
                {"path": "kept", "method": "GET", "summary": "a route still needed"},
            ],
            "submission_format": {"kind": "tar", "max_bytes": 1},
            "pin_policy": {"epsilon_nll_min": 0.02},
        }

    def test_a_re_authoring_run_retains_the_parts_it_is_not_changing(self):
        h = Harness(topic_document(), previous=self.previous_set())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        set_ = h.authored()
        names = [m["name"] for m in set_["migrations"]]
        self.assertIn("0001_scratch", names, "a migration the topic still needs must survive")
        self.assertIn("0002_kept", names, "a retained migration is kept")
        self.assertIn("0001_rlm_state", names, "the RLM's current answer is added")
        self.assertEqual(
            names[: len(self.previous_set()["migrations"])],
            ["0001_scratch", "0002_kept"],
            "prior order is preserved",
        )
        routes = [(a["path"], a["method"]) for a in set_["apis"]]
        self.assertIn(("kept", "GET"), routes)
        self.assertIn(("status", "GET"), routes)
        # The current answer wins for the route it names.
        status = next(a for a in set_["apis"] if a["path"] == "status")
        self.assertNotEqual(status["summary"], "the old summary")

    def test_a_retained_migration_is_still_scope_checked(self):
        """Retention is not a bypass: a prior set is not a trusted input."""
        prior = self.previous_set()
        prior["migrations"].append(
            {"name": "0003_evil", "sql": "DROP TABLE proof_rule_version"}
        )
        h = Harness(topic_document(), previous=prior)
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()
        self.assertFalse(h.set_path().exists())

    def test_the_submission_format_is_re_derived_not_retained(self):
        """A retained format would publish a previous host's intake contract.

        Greptile P1: a populated prior `submission_format` was copied into the
        new set, so a re-authoring run published the old contract (here a
        1-byte cap) as the current one.
        """
        prior = self.previous_set()
        prior["submission_format"] = {
            "kind": "tar",
            "max_bytes": 1,
            "stale_marker": "prior-host-contract",
        }
        h = Harness(topic_document(), previous=prior)
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        fmt = h.authored()["submission_format"]
        self.assertNotEqual(fmt, prior["submission_format"], "the stale contract was retained")
        self.assertNotIn("stale_marker", fmt)
        self.assertEqual(fmt["max_bytes"], authoring_set.MAX_ARTIFACT_BYTES)
        self.assertEqual(fmt["signature_domain"], authoring_set.SUBMIT_DOMAIN)
        self.assertEqual(fmt, authoring_set.derive_submission_format())

    def test_a_retained_topic_scoped_delete_is_not_refused(self):
        """`DELETE FROM <topic>_table` is in the namespace, not a touch on FROM.

        Greptile P1: the scan skipped modifiers after a table keyword but not
        `FROM`, so a retained `DELETE FROM fixture_topic_v0_kept …` was refused
        with "migration touches 'FROM'".
        """
        prior = self.previous_set()
        prefix = TOPIC_ID.replace("-", "_")
        prior["migrations"].append(
            {
                "name": "0003_prune",
                "sql": f"DELETE FROM {prefix}_kept WHERE id = 'retained-row'",
            }
        )
        h = Harness(topic_document(), previous=prior)
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        names = [m["name"] for m in h.authored()["migrations"]]
        self.assertIn("0003_prune", names, "the retained DELETE migration must survive")
        # And the scope check itself, directly, for both spellings.
        authoring_set.check_migration_scope(
            f"DELETE FROM {prefix}_kept WHERE id = 'x'", TOPIC_ID
        )
        authoring_set.check_migration_scope(
            f'DELETE FROM "{TOPIC_ID}_kept" WHERE id = \'x\'', TOPIC_ID
        )
        authoring_set.check_migration_scope(
            f"DELETE FROM {prefix}_kept USING {prefix}_other WHERE 1 = 1", TOPIC_ID
        )
        authoring_set.check_migration_scope(
            f"UPDATE {prefix}_kept SET value = 'x' WHERE id = 'y'", TOPIC_ID
        )
        authoring_set.check_migration_scope(
            f"TRUNCATE {prefix}_kept", TOPIC_ID
        )
        # A DELETE that reaches a sibling is still refused.
        with self.assertRaises(SystemExit):
            authoring_set.check_migration_scope("DELETE FROM other_topic_rows", TOPIC_ID)
        with self.assertRaises(SystemExit):
            authoring_set.check_migration_scope(
                "DELETE FROM proof_rule_version WHERE id = 'x'", TOPIC_ID
            )

    def test_the_rules_and_the_policy_are_re_derived_not_retained(self):
        """A retained rule or policy could contradict the re-signed document."""
        h = Harness(topic_document(), previous=self.previous_set())
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        set_ = h.authored()
        self.assertNotIn(
            "stale_rule",
            [r["id"] for r in set_["rules"]],
            "a rule the re-signed topic dropped must not be retained",
        )
        self.assertNotEqual(set_["pin_policy"], {"epsilon_nll_min": 0.02})
        self.assertEqual(set_["pin_policy"]["epsilon_nll_min"], 0.02)
        self.assertIn("max_proof_deadline_s", set_["pin_policy"])


class Bounds(unittest.TestCase):
    def test_the_rule_text_stays_within_the_signed_cap(self):
        long_text = "x" * 4000
        doc = topic_document(
            checklist=[{"id": "no_short_circuit", "text": long_text}]
        )
        h = Harness(doc, markers="no_short_circuit:skip_eval", attested="")
        self.addCleanup(h.cleanup)
        self.assertEqual(h.run(), 0)
        text = h.authored()["rules"][0]["text"]
        self.assertLessEqual(len(text), authoring_set.MAX_RULE_TEXT_CHARS)

    def test_too_many_rules_is_refused(self):
        ids = [f"rule_{i:02d}" for i in range(authoring_set.MAX_RULES + 1)]
        doc = topic_document(checklist=[{"id": rid, "text": "a rule"} for rid in ids])
        markers = ";".join(f"{rid}:marker" for rid in ids)
        h = Harness(doc, markers=markers, attested="")
        self.addCleanup(h.cleanup)
        with self.assertRaises(SystemExit):
            h.run()


if __name__ == "__main__":
    unittest.main(verbosity=2)
