# 70 — Secrets: BIP39 mnemonics, wallet JSON, agent audit logs

**Fail-closed. Non-negotiable.** A leaked owner hotkey mnemonic is a
subnet-control incident. Keystore already refuses a mnemonic from a plain env
var and will only read a file at mode `0600` or stricter. **That is not
enough.** The leak class this page exists to stop is: the phrase (or a wallet
JSON that contains it) appearing in a shell command, a GitHub Actions secret,
an agent audit log, or any other plaintext surface.

This page does not change validator burn / weights logic, does not invent a
chain swap, and does not weaken [`crates/keystore`](../crates/keystore).

## Hard rules

1. **NEVER paste, print, echo, cat, or log a BIP39 mnemonic, `secretPhrase`,
   or raw hotkey / coldkey seed.** Not in Chat, Slack, GitHub issues / PRs, CI
   logs, `docker inspect`, process argv, crash reports, or agent
   `audit.jsonl`. Do not put one in a commit, a comment, a test fixture, or a
   log line — including as a "redacted" example that is still a real phrase.

2. **NEVER pass a mnemonic as a CLI argument or embed it in a
   `shell_command`.** Write it only to a file created with `umask 077` / mode
   `0600`, then pass the **path** (`BASE_VALIDATOR_MNEMONIC_FILE`,
   `BASE_GATEWAY_MNEMONIC_FILE`, and the matching `{PREFIX}_MNEMONIC_FILE`
   keys). The process environment may hold the path. It must never hold the
   phrase.

3. **NEVER store mnemonics in GitHub Actions secrets, workflow `env:`,
   cloud-init, Terraform state, or Compose `environment:`.** GitHub secret
   *names* must not be used as a mnemonic transport.
   `PROD_ROTATE_MNEMONIC` is **banned**. Do not reintroduce it, and do not
   rename it into another `*MNEMONIC*` Actions secret. `rules-check` fails a
   workflow that does.

4. **Wallet JSON with `secretPhrase` is as sensitive as the phrase.** Same
   rules: no argv, no Chat, no CI secret, no 644 file, no compose env value.
   `btcli` hotkey files under `deploy/secrets/wallets/` stay gitignored, mode
   `0400`, owner uid `65532` on hosts.

5. **Agent audit logs must be mode `0600` and must redact mnemonic /
   `secretPhrase` if a command would contain them.** A `644` `audit.jsonl`
   that captured a wallet write is a leak. Fix the mode and the redaction;
   deleting the file after the fact does not un-leak it.

6. **No mnemonic files in git.** `.dockerignore` already has `**/*mnemonic*`;
   keep that. `.gitignore` already has `**/*mnemonic*` and keeps
   `deploy/secrets/` untracked except `README.md`. The one gitignore
   exception is `!.rules/70-secrets-mnemonics.md` — this page is
   documentation, not a secret. Do not force-add wallet files.

## Allowed (path only)

| Do | Do not |
|----|--------|
| `umask 077`; `install -m 0600 /dev/null <path>`; write the phrase with an editor bound to that path | `echo` / `printf` / `cat` of the phrase (argv + shell history) |
| `BASE_VALIDATOR_MNEMONIC_FILE=/run/base/validator_mnemonic` | `BASE_VALIDATOR_MNEMONIC='…phrase…'` (keystore refuses this; do not add it) |
| Age-encrypted material under `deploy/secrets/`, decrypted to mode `0600` | Baking the phrase into an image, cloud-init, or TF state |
| Mount the **file** into compose (`:ro`); env holds the path | Compose `environment: SECRET_PHRASE: …` or `environment: PROD_ROTATE_MNEMONIC: …` |
| Rotate by writing a new 0600 file on the host, then pointing the path at it | SCP of wallet JSON driven by a GitHub Actions mnemonic secret |

Keystore resolution order (do not weaken): Bittensor wallet dir →
`{PREFIX}_MNEMONIC_FILE` (mode `0600` or stricter) → `{PREFIX}_SK_FILE`.
Mnemonics are deliberately **not** read from a plain environment variable:
process environments leak through `docker inspect`, `/proc/<pid>/environ`,
and crash reporters. Errors from the keystore crate must never embed the
phrase.

## Incident class this page encodes

Two independent failures produced a live SN100 owner-hotkey leak:

1. An agent wrote the phrase into a shell command. That command was recorded
   **plaintext** in `audit.jsonl` at mode `644`.
2. A GitHub Actions secret named `PROD_ROTATE_MNEMONIC` was used to SCP wallet
   JSON containing `secretPhrase` onto prod hosts.

Neither is a keystore bug. Both are forbidden here. Do not reconstruct the
phrase in this tree, in a test, or in a PR comment.

## Machine check

```bash
cargo run -p xtask -- rules-check
```

Fails if this file is missing, if [`.rules/00-overview.md`](00-overview.md)
stops naming it, if [`AGENTS.md`](../AGENTS.md), [`40-agents.md`](40-agents.md),
or [`contracts/THREAT_MODEL.md`](contracts/THREAT_MODEL.md) stop linking it, if
`.dockerignore` drops `**/*mnemonic*`, if `.gitignore` drops that glob or the
`!.rules/70-secrets-mnemonics.md` exception, or if a GitHub workflow uses
`PROD_ROTATE_MNEMONIC` or any `secrets.*MNEMONIC*` transport.
