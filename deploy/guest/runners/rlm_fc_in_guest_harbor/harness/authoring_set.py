#!/usr/bin/env python3
"""Author the topic's whole behavior set — the RLM's answer to `ProposeRules`.

Writes ``$PROOF_OUTPUT_DIR/authoring.json``: ``schema_version`` 1 plus the five
parts an install applies. **Never** writes ``rules.json``: a rules-only answer
is a fragment, the host records it honestly and refuses to open the topic
(``SetupError::IncompleteAuthoring`` / ``RULES_ONLY_IS_NOT_AUTHORSHIP``), and a
fragment is not this entrypoint's job.

What each part is authored *from*, and why it is the RLM's answer rather than
a copy of the operator's bundle:

===============================  =========================================
part                             source of the answer
===============================  =========================================
``rules``                        the signed ``checklist`` **partitioned by
                                 what this RLM can actually tick**: only a
                                 rule the topic names in
                                 ``inspect_marker_rules`` /
                                 ``inspect_attested_rules`` is enforceable,
                                 and the vector is the RLM's own framing of
                                 it. A declared rule with no policy is a
                                 refusal, never a silent drop — dropping it
                                 would narrow the topic's anti-cheat
                                 surface behind the operator's back.
``migrations``                   the topic's own namespace (its id, ``-`` →
                                 ``_``), because a complete set needs at
                                 least one and the RLM's minimum schema is
                                 its own state table. Prior entries the
                                 derivation does not name are **retained**.
``apis``                         the topic's own prefix; the mux serves the
                                 row, so the RLM authors the route it
                                 exposes and no handler it cannot have.
                                 Prior routes are retained the same way.
``submission_format``            what this RLM's runtime accepts: the host's
                                 real intake shape (uncompressed tar under
                                 the staged cap, sr25519 hotkey signature
                                 over the submit domain, single-use nonce),
                                 never a bundle section.
``pin_policy``                   a **restatement** of the signed document's
                                 own knobs. Scoring reads the document, so a
                                 policy may restate what the document
                                 declares — proving the RLM considered the
                                 knob — and may not diverge from it in
                                 either direction. Knobs the RLM cannot
                                 truthfully name (``eval_image_digest``,
                                 ``gpu_class``: equalities against the
                                 **pin**, which the VM does not hold) are
                                 left absent rather than invented.
===============================  =========================================

**Re-authoring retains what it is not changing.** ``PROOF_CURRENT_AUTHORING_FILE``
(e.g. ``current-authoring.json``; empty on a first run) carries the set this
RLM authored last time. ``migrations`` and ``apis`` are merged: the prior order
is preserved, the RLM's current answer replaces the entries it names, and
entries it does not name are kept — without that a re-authoring run is a
rewrite from nothing, and a migration the topic still needs would silently
vanish.

``rules``, ``submission_format`` and ``pin_policy`` are **always re-derived**,
because each is a fact about *now* rather than a decision to keep:

* a retained rule could be one the re-signed document dropped (the vector is
  the topic's anti-cheat surface, and it must match the declaration);
* a retained ``submission_format`` would publish a **previous host's** intake
  contract — the staged cap, the submit domain, the nonce are properties of
  the runtime this run is executing on, so they are re-read every run;
* a retained policy could diverge from the document that scoring actually
  reads.

**What is checked here, and what is not.** The authoritative gates are the
guest's (``crates/proof-vm-guest``, the same ``proof-topic-authoring`` the
install links) and the install's SQL deny-list. This module holds the set to
the same shape before writing it, so a malformed answer fails inside the VM
with the part named instead of becoming a job output. The migration check here
is deliberately conservative rather than complete: it refuses an unscoped or
``proof_*`` name and leaves the full deny-list to the guard that owns it.

Secrets: ``propose_rules`` is unpaid and reads none. Nothing here is logged.
"""

from __future__ import annotations

import json
import math
import os
import re
import sys
import tempfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
# `inspect_scan` is the adaptor's own inspector. Reusing its parser is what
# makes "the RLM authors the vector its inspector ticks" true by construction:
# a rule this file authors is tickable by the code that will tick it, and the
# two cannot drift.
sys.path.insert(0, str(HERE.parent))
import inspect_scan  # noqa: E402

AUTHORING_SCHEMA = 1
AUTHORING_FILE = "authoring.json"
CURRENT_AUTHORING_FILE = "current-authoring.json"

MAX_RULES = 64
MAX_RULE_TEXT_CHARS = 2048
MAX_MIGRATIONS = 64
MAX_MIGRATION_SQL_BYTES = 256 * 1024
MAX_APIS = 64
MAX_API_SUMMARY_CHARS = 256
MAX_PIN_STRING_CHARS = 256

# `proof_vm_proto::guest::MAX_STAGED_ARTIFACT_TAR_BYTES`: the host refuses a
# larger staged tar, so a format claiming more would be a promise it does not
# keep.
MAX_ARTIFACT_BYTES = 5 * 1024 * 1024
# `base-proof-submit-v1`: the domain the miner's hotkey signature is over.
SUBMIT_DOMAIN = "base-proof-submit-v1"

RULE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{1,63}$")
API_METHODS = ("GET", "POST", "PUT", "PATCH", "DELETE", "*")
# `proof_topic_authoring::RESERVED_API_PREFIXES`: the challenge's own operator
# surface, which is not a topic's to claim.
RESERVED_API_PREFIXES = ("v1/admin",)
# `proof_topic_sql_guard`: the prefix every object this repository owns
# carries. A topic migration may not name one, whatever the verb.
OWNED_TABLE_PREFIX = "proof_"
TABLE_KEYWORDS = ("FROM", "JOIN", "INTO", "UPDATE", "TABLE", "INDEX", "TRUNCATE", "DELETE")
# `proof_topic_sql_guard::is_sql_keyword`: tokens that are never a table name in
# the position the scan reads. `FROM` is here for the same reason it is in the
# guard — in `DELETE FROM x` the token after `DELETE` is `FROM`, and the table
# is the token after *that* (which the scan reaches because `FROM` is itself a
# table keyword). Without this a retained `DELETE FROM <topic-scoped table>`
# is refused for "touching FROM".
SQL_KEYWORDS = (
    "select",
    "from",
    "where",
    "values",
    "set",
    "and",
    "or",
    "not",
    "null",
    "default",
    "lateral",
    "unnest",
    "true",
    "false",
)
# Modifiers skipped between a table keyword and the name it introduces.
TABLE_MODIFIERS = (
    "IF",
    "NOT",
    "EXISTS",
    "OR",
    "REPLACE",
    "ONLY",
    "INTO",
    "UNIQUE",
    "CONCURRENTLY",
)
DENIED_OBJECTS = (
    "_sqlx_migrations",
    "base_app",
    "pg_roles",
    "pg_authid",
    "information_schema",
    "pg_catalog",
    "pg_proc",
    "pg_shadow",
)

PARAM_MARKER_RULES = inspect_scan.PARAM_MARKER_RULES
PARAM_ATTESTED_RULES = inspect_scan.PARAM_ATTESTED_RULES


def _fail(msg: str, code: int = 2) -> None:
    print(f"authoring_set: {msg}", file=sys.stderr)
    raise SystemExit(code)


def _load_json(path: Path, what: str) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        _fail(f"cannot read {what} from {path}: {e}")


def _as_object(value: Any, what: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        _fail(f"{what} is not a JSON object")
    return value


def _clip(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    return text[: limit - 3] + "..."


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_fraction(value: Any) -> bool:
    """A finite knob in `(0, 1]` — the shape a pin-policy floor may carry."""
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and 0.0 < value <= 1.0
    )


def topic_document() -> dict[str, Any]:
    path = os.environ.get("PROOF_TOPIC_FILE", "").strip()
    if not path:
        _fail("PROOF_TOPIC_FILE is required")
    doc = _as_object(_load_json(Path(path), "the signed topic"), "the signed topic")
    topic_id = doc.get("id")
    if not isinstance(topic_id, str) or not topic_id.strip():
        _fail("the signed topic carries no id; the set cannot be bound to a topic")
    return doc


def current_set() -> dict[str, Any] | None:
    """The set this RLM authored last time, or ``None`` on a first run.

    The guest always sets ``PROOF_CURRENT_AUTHORING_FILE`` (empty when there is
    none) so this branches on one variable rather than on its presence. A file
    that exists but cannot be read is a refusal: silently rewriting from
    nothing is the lossy re-authoring the retention exists to prevent.
    """
    path = os.environ.get("PROOF_CURRENT_AUTHORING_FILE", "").strip()
    if not path:
        return None
    p = Path(path)
    if not p.is_file():
        _fail(f"PROOF_CURRENT_AUTHORING_FILE={path} is not a file")
    return _as_object(_load_json(p, "the previous authored set"), "the previous authored set")


def sql_prefix(topic_id: str) -> str:
    """`proof_topic_sql_guard::topic_sql_prefix`: the id's identifier-safe form."""
    return topic_id.strip().lower().replace("-", "_")


def is_topic_scoped(name: str, topic_id: str) -> bool:
    """`proof_topic_sql_guard::is_topic_scoped`, in Python.

    Two spellings are the topic's and only two: a `{id}`-qualified name
    (`<id>.scores`) or a bare `{id}_`-prefixed one (`<id>_scores`), with the
    hyphen id's identifier-safe form accepted in both positions.
    """
    n = name.strip().strip('"').lower()
    if not n:
        return False
    topic = topic_id.strip().lower()
    mapped = sql_prefix(topic)
    schema, _, bare = n.partition(".")
    if not bare:
        schema, bare = "", schema
    if schema in (topic, mapped):
        return True
    return bare.startswith(f"{topic}_") or bare.startswith(f"{mapped}_")


# ---------------------------------------------------------------------------
# rules
# ---------------------------------------------------------------------------


def derive_rules(doc: dict[str, Any]) -> list[dict[str, str]]:
    """The vector this RLM ticks, framed by the RLM.

    The topic's signed policy is what makes a rule *tickable*:
    ``inspect_marker_rules`` names a rule this RLM proves by scanning the
    artefact for off-limits markers, ``inspect_attested_rules`` one the host or
    topic enforces outside the scan. The signed ``checklist`` is the
    operator's declaration of intent; it seeds the ids and the order.

    The authored **text** is the RLM's own statement of what it will show and
    how it will show it, with the topic's sentence quoted as the declaration it
    enforces. That is the part the RLM owns: the operator said what the rule
    means, the RLM says what it will prove.

    The vector is exactly the declared rules, in declaration order — no rule is
    added and none is dropped, because the vector in force is the inspection
    surface miners verified against. Two refusals follow from that:

    * a declared rule with **no** policy — this RLM will not invent a check for
      a rule it was handed, and it will not drop one either: dropping it would
      narrow the topic's anti-cheat surface without saying so. (Left in, the
      inspector would record it red forever and the topic could never open.)
    * a **marker** rule the checklist does not declare — a signed marker policy
      for a rule the topic does not carry is a check that would never run, and
      silently ignoring it would discard something the operator asked for. An
      *attested* id outside the checklist is not a refusal: the inspector never
      ticks a rule that is not in the vector, so a host fact the topic declares
      and does not carry as a rule is simply not one of this RLM's rules.
    """
    declared: list[dict[str, str]] = []
    for item in doc.get("checklist") or []:
        if not isinstance(item, dict):
            _fail("checklist carries an entry that is not an object")
        rid = item.get("id")
        if not isinstance(rid, str) or not rid.strip():
            _fail("checklist carries a rule with no id")
        text = item.get("text")
        declared.append(
            {"id": rid.strip(), "text": text.strip() if isinstance(text, str) else ""}
        )
    declared_ids = {r["id"] for r in declared}

    # The adaptor's own parser: a policy it accepts here is one its inspector
    # accepts at tick time. Both fail closed on a malformed entry.
    marker_rules = inspect_scan.parse_marker_rules(os.environ.get(PARAM_MARKER_RULES))
    attested = inspect_scan.parse_attested_rules(os.environ.get(PARAM_ATTESTED_RULES))
    both = sorted(set(marker_rules) & attested)
    if both:
        _fail(
            f"{PARAM_MARKER_RULES} and {PARAM_ATTESTED_RULES} both name {', '.join(both)}; "
            "a rule is ticked one way"
        )
    if not marker_rules and not attested:
        _fail(
            f"the signed topic names no rule policy: set {PARAM_MARKER_RULES} (rules this RLM "
            f"proves by scanning the artefact) and/or {PARAM_ATTESTED_RULES} (rules the host or "
            "topic enforces outside the scan) and re-run. Without a policy every rule the "
            "inspector ticks is red, so the topic could never open — this entrypoint does not "
            "invent a check for a rule the topic never said how to tick."
        )
    if not declared:
        _fail(
            "the signed topic declares no checklist rules, so this RLM has no rule vector to "
            "author: the vector is the topic's anti-cheat surface, and inventing one would "
            "score miners against rules they never verified. Sign the topic's checklist (and "
            "the inspect policy for each rule) and re-run."
        )

    unpolicy = [r["id"] for r in declared if r["id"] not in marker_rules and r["id"] not in attested]
    if unpolicy:
        _fail(
            f"the signed topic declares {', '.join(unpolicy)} but names them in neither "
            f"{PARAM_MARKER_RULES} nor {PARAM_ATTESTED_RULES}; this RLM ticks what the topic "
            "says it is ticked by. Re-sign the topic with a policy for every rule it declares "
            "(or drop the rule from the checklist)"
        )
    orphan_markers = sorted(rid for rid in marker_rules if rid not in declared_ids)
    if orphan_markers:
        _fail(
            f"{PARAM_MARKER_RULES} names {', '.join(orphan_markers)}, which the signed checklist "
            "does not declare; a marker check for a rule the topic does not carry is a typo"
        )

    rules: list[dict[str, str]] = []
    for item in declared:
        rid, quoted = item["id"], item["text"]
        if rid in marker_rules:
            show = (
                f"none of the {len(marker_rules[rid])} off-limits markers the signed topic names "
                "for this rule appear in the miner's artefact, in its text or its file names (a "
                "truncated scan is not a pass)"
            )
            how = "rlm: artefact scan, no inference"
        else:
            show = (
                "the host or topic enforces this outside the artefact scan, and this RLM records "
                "the signed attestation instead of scanning"
            )
            how = "rlm: host/topic attestation, no inference"
        text = f"{show} [{how}]"
        if quoted:
            text += f" — the topic's declaration: {quoted}"
        rules.append({"id": rid, "text": _clip(text, MAX_RULE_TEXT_CHARS)})
    if len(rules) > MAX_RULES:
        _fail(f"the vector would carry {len(rules)} rules; at most {MAX_RULES} are applied")
    return rules


# ---------------------------------------------------------------------------
# migrations
# ---------------------------------------------------------------------------


def derive_migrations(doc: dict[str, Any]) -> list[dict[str, str]]:
    """The RLM's minimum schema, inside the topic's own namespace.

    One table, because a complete set carries at least one migration and this
    is the schema the RLM's runtime keeps for itself: the topic's own state,
    under the topic's prefix. It touches nothing else — the shared database's
    objects are the repository's, and the deny-list refuses them.
    """
    prefix = sql_prefix(doc["id"].strip())
    sql = (
        f"CREATE TABLE {prefix}_rlm_state (\n"
        "    key TEXT PRIMARY KEY,\n"
        "    value TEXT NOT NULL,\n"
        "    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()\n"
        ")"
    )
    return [{"name": "0001_rlm_state", "sql": sql}]


def merge_migrations(
    derived: list[dict[str, str]], retained: Any
) -> list[dict[str, str]]:
    """Prior order preserved; the RLM's answer wins per name; the rest kept."""
    prior: list[dict[str, str]] = []
    if retained is not None:
        if not isinstance(retained, list):
            _fail("the previous set's migrations are not a list")
        for item in retained:
            if not isinstance(item, dict):
                _fail("the previous set carries a migration that is not an object")
            prior.append(item)
    current = {m["name"]: m for m in derived}
    merged: list[dict[str, str]] = []
    seen: set[str] = set()
    for item in prior:
        name = item.get("name")
        if not isinstance(name, str) or not name.strip():
            _fail("the previous set carries a migration with no name")
        if name in seen:
            _fail(f"the previous set names the migration {name!r} twice")
        seen.add(name)
        merged.append(current.get(name, item))
    for name, item in current.items():
        if name not in seen:
            merged.append(item)
    if len(merged) > MAX_MIGRATIONS:
        _fail(f"the set would carry {len(merged)} migrations; at most {MAX_MIGRATIONS} are applied")
    return merged


IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_$]*(?:\.[A-Za-z_][A-Za-z0-9_$]*)*")
QUOTED_IDENTIFIER = re.compile(r'"([^"]*)"')


def _identifiers(sql: str) -> list[str]:
    """Identifiers in source order, double-quoted runs kept whole.

    `proof_topic_sql_guard::tokens` does the same, and for the same reason: a
    hyphenated topic's mapped name is one identifier, and the **literal** id
    (`"fixture-topic-v0_scratch"`) is a single legal quoted identifier that
    splitting at the hyphen would turn into three unscoped ones. Order matters
    — the namespace check reads the name *after* a table keyword — so a quoted
    run is emitted where it appears, not collected separately.
    """
    out: list[str] = []
    cursor = 0
    for match in QUOTED_IDENTIFIER.finditer(sql):
        out.extend(IDENTIFIER.findall(sql[cursor : match.start()]))
        out.append(match.group(1))
        cursor = match.end()
    out.extend(IDENTIFIER.findall(sql[cursor:]))
    return out


def check_migration_scope(sql: str, topic_id: str) -> None:
    """Refuse a name that is not inside the topic's namespace.

    Conservative on purpose — the authoritative deny-list is
    `proof_topic_sql_guard`, which runs in the guest and again at install.
    This is the pre-flight that keeps an obviously unscoped migration out of a
    job output, and it checks the same object positions that guard checks:
    every identifier for the owned/denied names, then the name after each
    table keyword for the namespace.
    """
    for token in _identifiers(sql):
        base = token.rsplit(".", 1)[-1].lower()
        if base.startswith(OWNED_TABLE_PREFIX):
            _fail(
                f"migration names {token!r}: every proof_* object belongs to the scoring path "
                "and is not a topic's to touch"
            )
        if base in DENIED_OBJECTS or token.lower() in DENIED_OBJECTS:
            _fail(f"migration names {token!r}: the shared database's own objects are not a topic's")
    words = _identifiers(sql)
    for index, word in enumerate(words):
        if word.upper() not in TABLE_KEYWORDS:
            continue
        cursor = index + 1
        while cursor < len(words) and words[cursor].upper() in TABLE_MODIFIERS:
            cursor += 1
        if cursor >= len(words):
            continue
        name = words[cursor]
        # A token that is itself a SQL keyword is never a table name in this
        # position — `DELETE FROM x` reads `FROM` here, and the outer loop
        # reaches `x` because `FROM` is a table keyword too. Refusing it would
        # reject every topic-scoped `DELETE FROM <topic>_table`.
        if name.lower() in SQL_KEYWORDS:
            continue
        if name.lower().startswith(OWNED_TABLE_PREFIX):
            continue  # already refused above, by identifier
        if not is_topic_scoped(name, topic_id):
            _fail(
                f"migration touches {name!r}, which is not inside the topic's namespace "
                f"({sql_prefix(topic_id)}_* / {sql_prefix(topic_id)}.*); an unscoped name would "
                "collide with — or read — another topic's install"
            )


def check_migration(item: Any) -> dict[str, str]:
    if not isinstance(item, dict):
        _fail("a migration is not an object")
    name = item.get("name")
    sql = item.get("sql")
    if not isinstance(name, str) or not RULE_ID.match(name.strip()):
        _fail(f"migration name {name!r} must match [a-z0-9][a-z0-9_-]{{1,63}}")
    if not isinstance(sql, str) or not sql.strip():
        _fail(f"migration {name!r} carries no SQL; remove it instead")
    if len(sql.encode("utf-8")) > MAX_MIGRATION_SQL_BYTES:
        _fail(f"migration {name!r} is larger than {MAX_MIGRATION_SQL_BYTES} bytes")
    return {"name": name.strip(), "sql": sql}


# ---------------------------------------------------------------------------
# apis
# ---------------------------------------------------------------------------


def derive_apis(doc: dict[str, Any]) -> list[dict[str, str]]:
    """The route this topic exposes for itself, under its own prefix.

    The mux serves the **row** the install wrote, so the RLM authors a route
    whose answer is its own record and never a handler it cannot have.
    """
    del doc
    return [
        {
            "path": "status",
            "method": "GET",
            "summary": (
                "the topic's own record: the route row this install registered under the topic's "
                "prefix, served by the challenge's registry"
            ),
        }
    ]


def merge_apis(derived: list[dict[str, str]], retained: Any) -> list[dict[str, str]]:
    """Same rule as migrations, keyed by `(path, method)`."""
    prior: list[dict[str, str]] = []
    if retained is not None:
        if not isinstance(retained, list):
            _fail("the previous set's apis are not a list")
        for item in retained:
            if not isinstance(item, dict):
                _fail("the previous set carries a route that is not an object")
            prior.append(item)
    current = {(a["path"], a["method"]): a for a in derived}
    merged: list[dict[str, str]] = []
    seen: set[tuple[str, str]] = set()
    for item in prior:
        key = (str(item.get("path", "")), str(item.get("method", "")))
        if key in seen:
            _fail(f"the previous set names the route {key[1]} /{key[0]} twice")
        seen.add(key)
        merged.append(current.get(key, item))
    for key, item in current.items():
        if key not in seen:
            merged.append(item)
    if len(merged) > MAX_APIS:
        _fail(f"the set would carry {len(merged)} routes; at most {MAX_APIS} are applied")
    return merged


def check_api(item: Any) -> dict[str, str]:
    if not isinstance(item, dict):
        _fail("a route is not an object")
    path = item.get("path")
    method = item.get("method")
    summary = item.get("summary", "")
    if not isinstance(path, str) or not _is_relative_api_path(path):
        _fail(
            f"route path {path!r} must be a relative path of plain segments (no leading '/', no "
            "'..', no empty segment): a topic's routes live under its own prefix"
        )
    if any(
        path == prefix or path.startswith(f"{prefix}/")
        for prefix in RESERVED_API_PREFIXES
    ):
        _fail(
            f"route path {path!r} is inside the challenge's admin namespace "
            f"({', '.join(RESERVED_API_PREFIXES)}), which is not a topic's to claim"
        )
    if not isinstance(method, str) or method.strip().upper() not in API_METHODS:
        _fail(f"route method {method!r} must be one of {', '.join(API_METHODS)}")
    if not isinstance(summary, str):
        _fail(f"route {path!r} carries a summary that is not a string")
    if len(summary) > MAX_API_SUMMARY_CHARS:
        _fail(f"route {path!r} carries a summary longer than {MAX_API_SUMMARY_CHARS} chars")
    return {"path": path, "method": method.strip().upper(), "summary": summary}


def _is_control(c: str) -> bool:
    return ord(c) < 0x20 or ord(c) == 0x7F


def _is_relative_api_path(path: str) -> bool:
    """`proof_topic_authoring::is_relative_api_path`, in Python."""
    p = path.strip()
    if not p or len(p) > 512 or p.startswith("/") or p.endswith("/"):
        return False
    if any(_is_control(c) or c in "\\?#" for c in p):
        return False
    segments = p.split("/")
    if any(seg in ("", ".", "..") for seg in segments):
        return False
    allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._~-")
    return all(set(seg) <= allowed for seg in segments)


# ---------------------------------------------------------------------------
# submission_format / pin_policy
# ---------------------------------------------------------------------------


def derive_submission_format() -> dict[str, Any]:
    """What this RLM's runtime accepts, as the host's intake actually is."""
    return {
        "kind": "tar",
        "compression": "none",
        "max_bytes": MAX_ARTIFACT_BYTES,
        "artifact_digest": "sha256 of the exact served bytes; the guest verifies what it fetched",
        "identity": "sr25519 hotkey_signature over the canonical submit payload",
        "signature_domain": SUBMIT_DOMAIN,
        "replay": "single-use 64-hex submit_nonce; a replay is refused before any row",
        "artifact": "multipart part `artifact` (bytes win) or artifact_uri (the served file, verbatim)",
        "miner_supplies": "claim + code + artifact; the judge and executor offers are not the miner's to bind",
    }


def derive_pin_policy(doc: dict[str, Any]) -> dict[str, Any]:
    """Restate the signed document's own knobs, and nothing it does not declare.

    Every knob here is an **equality** with the document (`PinPolicy::agrees_with_document`):
    scoring reads the document, so a policy that named a different number would
    be a threshold no challenger is judged by. The knobs that are equalities
    against the **pin** (`eval_image_digest`, `gpu_class`) are left absent —
    this VM does not hold the pin, and a value it cannot read is not a value it
    may invent.
    """
    policy: dict[str, Any] = {}
    for key, source in (
        ("epsilon_nll_min", doc.get("epsilon_nll")),
        ("epsilon_topic_max_regress_min", doc.get("epsilon_topic_max_regress")),
        ("epsilon_throughput_rel_min", (doc.get("metric") or {}).get("epsilon_rel")),
    ):
        if _is_fraction(source):
            policy[key] = source
    deadline = (doc.get("eval_executor") or {}).get("max_proof_deadline_s")
    if _is_int(deadline) and deadline >= 1:
        policy["max_proof_deadline_s"] = deadline
    budget = doc.get("flops_budget")
    if _is_int(budget) and budget >= 1:
        policy["flops_budget_max"] = budget
    holdout = doc.get("holdout_size")
    if _is_int(holdout) and holdout >= 1:
        policy["holdout_size"] = holdout
    return policy


def check_pin_policy(item: Any) -> dict[str, Any]:
    if not isinstance(item, dict):
        _fail("pin_policy is not an object")
    allowed = {
        "epsilon_nll_min",
        "epsilon_throughput_rel_min",
        "epsilon_topic_max_regress_min",
        "max_proof_deadline_s",
        "flops_budget_max",
        "holdout_size",
        "eval_image_digest",
        "gpu_class",
    }
    unknown = sorted(set(item) - allowed)
    if unknown:
        _fail(f"pin_policy carries {', '.join(unknown)}, which this build does not read")
    for key in (
        "epsilon_nll_min",
        "epsilon_throughput_rel_min",
        "epsilon_topic_max_regress_min",
    ):
        if key in item and not _is_fraction(item[key]):
            _fail(f"pin_policy.{key} must be a finite fraction in (0, 1]")
    for key in ("max_proof_deadline_s", "flops_budget_max", "holdout_size"):
        if key in item and (not _is_int(item[key]) or item[key] < 1):
            _fail(f"pin_policy.{key} must be an integer >= 1")
    for key in ("eval_image_digest", "gpu_class"):
        if key in item:
            value = item[key]
            if not isinstance(value, str) or not value.strip():
                _fail(f"pin_policy.{key} must be a non-empty string")
            if len(value) > MAX_PIN_STRING_CHARS:
                _fail(f"pin_policy.{key} is longer than {MAX_PIN_STRING_CHARS} chars")
    return item


# ---------------------------------------------------------------------------
# the set
# ---------------------------------------------------------------------------


def build_set(doc: dict[str, Any], previous: dict[str, Any] | None) -> dict[str, Any]:
    """The whole set: the RLM's answer, with the prior set's parts retained."""
    topic_id = doc["id"].strip()
    prior = previous or {}
    if prior and isinstance(prior.get("topic_id"), str) and prior["topic_id"].strip() != topic_id:
        _fail(
            f"the previous set is for topic {prior['topic_id']!r}, this VM is bound to "
            f"{topic_id!r}; it is not this RLM's to retain"
        )
    migrations = merge_migrations(derive_migrations(doc), prior.get("migrations"))
    for item in migrations:
        check_migration(item)
        check_migration_scope(item["sql"], topic_id)
    apis = merge_apis(derive_apis(doc), prior.get("apis"))
    apis = [check_api(item) for item in apis]
    return {
        "schema_version": AUTHORING_SCHEMA,
        "topic_id": topic_id,
        "rules": derive_rules(doc),
        "migrations": migrations,
        "apis": apis,
        # **Always derived.** `submission_format` states the intake contract of
        # the host this run is executing on — the staged cap, the submit
        # domain, the nonce — and a retained copy would be a previous host's
        # contract published as the current one. Unlike a migration (which the
        # topic still needs and the RLM therefore keeps), this part is a fact
        # about the runtime, so it is re-read every run and never inherited.
        "submission_format": derive_submission_format(),
        "pin_policy": check_pin_policy(derive_pin_policy(doc)),
    }


def check_set(set_: dict[str, Any], doc: dict[str, Any]) -> None:
    """The guest's own gates, before the answer becomes a job output."""
    topic_id = doc["id"].strip()
    if set_.get("schema_version") != AUTHORING_SCHEMA:
        _fail(
            f"authored set schema_version {set_.get('schema_version')!r}, this build writes "
            f"{AUTHORING_SCHEMA}"
        )
    if str(set_.get("topic_id", "")).strip() != topic_id:
        _fail(
            f"the set is for topic {set_.get('topic_id')!r}, this VM is bound to {topic_id!r}"
        )
    missing = [
        part
        for part in ("rules", "migrations", "apis", "submission_format")
        if not set_.get(part)
    ]
    if missing:
        _fail(
            f"the RLM authored no {', '.join(missing)}: the set is incomplete, so nothing is "
            "installed — a topic's behavior is authored by its own RLM (rules, migrations, apis, "
            "submission_format, pin_policy)"
        )
    if "pin_policy" not in set_:
        _fail("the set carries no pin_policy key; an empty policy is an answer, an absent one is not")
    rules = set_["rules"]
    if len(rules) > MAX_RULES:
        _fail(f"the vector carries {len(rules)} rules; at most {MAX_RULES} are applied")
    seen: set[str] = set()
    for rule in rules:
        rid = rule.get("id") if isinstance(rule, dict) else None
        text = rule.get("text") if isinstance(rule, dict) else None
        if not isinstance(rid, str) or not RULE_ID.match(rid):
            _fail(f"rule id {rid!r} must match [a-z0-9][a-z0-9_-]{{1,63}}")
        if rid in seen:
            _fail(f"rule id {rid!r} appears twice in the vector")
        seen.add(rid)
        if not isinstance(text, str) or not text.strip():
            _fail(f"rule {rid!r} carries no text")
        if len(text) > MAX_RULE_TEXT_CHARS:
            _fail(f"rule {rid!r} carries more than {MAX_RULE_TEXT_CHARS} chars")


def write_set(set_: dict[str, Any], output_dir: Path) -> Path:
    """Write `authoring.json` in one step: a partial set never lands."""
    output_dir.mkdir(parents=True, exist_ok=True)
    path = output_dir / AUTHORING_FILE
    body = json.dumps(set_, indent=2, sort_keys=False) + "\n"
    handle, tmp = tempfile.mkstemp(dir=str(output_dir), prefix=".authoring-", suffix=".json")
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as fh:
            fh.write(body)
            fh.flush()
            os.fsync(fh.fileno())
        os.chmod(tmp, 0o644)
        os.replace(tmp, path)
    except OSError as e:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        _fail(f"cannot write {path}: {e}")
    return path


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv:
        _fail(f"propose_rules takes no arguments (got {' '.join(argv)})")
    output_dir = os.environ.get("PROOF_OUTPUT_DIR", "").strip()
    if not output_dir:
        _fail("PROOF_OUTPUT_DIR is required")
    job = os.environ.get("PROOF_JOB", "").strip()
    if job != "propose_rules":
        _fail(f"PROOF_JOB is {job!r}, not propose_rules")
    doc = topic_document()
    set_ = build_set(doc, current_set())
    check_set(set_, doc)
    path = write_set(set_, Path(output_dir))
    print(
        f"authoring_set: authored {len(set_['rules'])} rules, "
        f"{len(set_['migrations'])} migrations, {len(set_['apis'])} routes for "
        f"topic {set_['topic_id']} -> {path}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
