#!/usr/bin/env bash
# Limitations:
# - The secret scan skips digit-free values of 20+ characters under a credential parent
#   (reduces false positives on plain words).
# - The scan is a heuristic, not a guarantee: the out-of-band token approval and a manual
#   review of the file list stay mandatory before any push.
# Usage: publish.sh [--dry-run] [--tag] --expect-token-file F   (env HYPERTRAIN_PUBLISH_APPROVED=<token>)
# Push needs token in F (written by orchestrator out of band). --tag needs a SECOND token:
# env HYPERTRAIN_TAG_APPROVED matching file HYPERTRAIN_TAG_TOKEN_FILE (default F.tag).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="${HYPERTRAIN_REPO:-CortexLM/hypertrain}"
DRY=0; TAG=0; TF=""; OUT="${HYPERTRAIN_PUBLISH_OUT:-}"
while [ $# -gt 0 ]; do case "$1" in
  --dry-run) DRY=1;; --tag) TAG=1;; --expect-token-file) TF="$2"; shift;;
  --out) OUT="$2"; shift;; *) echo "unknown arg $1" >&2; exit 2;; esac; shift; done

list_files() { # allowlist = everything not matching .publishignore
  (cd "$ROOT" && find . -type f | sed 's|^\./||' | grep -Ev -f <(grep -Ev '^(#|$)' .publishignore) | LC_ALL=C sort)
}
scan() { # regex + entropy secret scan over the file list (single python pass); allowlist = literal substrings in .secretscan-allow
  list_files | (cd "$ROOT" && python3 -c '
import re,sys,math
allow=[l.strip() for l in open(".secretscan-allow") if l.strip() and not l.startswith("#")]
pats=[r"AKIA[0-9A-Z]{16}",r"-----BEGIN [A-Z ]*PRIVATE KEY-----",r"gh[pousr]_[A-Za-z0-9]{30,}",r"xox[abp]-[A-Za-z0-9-]{10,}",
 r"sk-[A-Za-z0-9]{32,}",r"FAL_KEY *[=:] *[\"\x27]?[A-Za-z0-9-]{20,}",r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:[0-9a-f]{32,}",
 r"fal_sk_[0-9a-fA-F]{20,}",r"fal_sk_[A-Za-z0-9]{16,}",r"(?i)(api|secret|token|key)[_-]?\w*[\"\x27]?\s*[:=]\s*[\"\x27]?[0-9a-f]{32,}(?![0-9a-f])",
 r"(instance_api_key|vast[_-]?(api[_-]?)?key|api[_-]?key|secret[_-]?key)[\"\x27 ]*[:=][\"\x27 ]*[A-Za-z0-9+/_-]{24,}"]
CW=r"token(?!iz)|secret|key|password|passwd|auth|credential|apikey"
CRED=re.compile(r"([\w.-]*(?:"+CW+r")[\w.-]*)[\"\x27]?\s*[:=]\s*[\"\x27]?(?:@?sha(?:256|512):)?[0-9a-fA-F]{32,}(?![0-9a-fA-F])",re.I)
bad=0
for f in sys.stdin.read().split("\n"):
    if not f or f.endswith((".u32",".png",".jpg",".lock")) or f==".secretscan-allow": continue
    stack=[]  # (indent, key) ancestors, indentation-aware
    for n,line in enumerate(open(f,errors="ignore"),1):
        km=re.match(r"^(\s*)[{\[,\s]*[\"\x27]?([\w.-]+)[\"\x27]?\s*:\s*(.*)$",line.rstrip("\n"))
        if km:
            ind=len(km.group(1)); key=km.group(2); val=km.group(3).strip().strip("{}[],").strip().strip("\"\x27")
            while stack and stack[-1][0]>=ind: stack.pop()
            if val[:7] in ("sha256:","sha512:") and not any(re.search(CW,k,re.I) for _,k in stack): pass
            elif re.fullmatch(r"[A-Za-z0-9+/_=-]{20,}",val) and re.search(r"\d",val) and any(re.search(CW,k,re.I) for _,k in stack) and not any(x in line for x in allow):
                print(f"SECRET {f}:{n} under credential parent {[k for _,k in stack]}"); bad=1
            stack.append((ind,key))
        for m in CRED.finditer(line):  # credential-word key names always block (hash/digest words or sha256: prefix never exempt)
            if not any(x in line for x in allow): print(f"SECRET {f}:{n} credential-named hex"); bad=1
        for p in pats:
            for m in re.finditer(p,line):
                if any(x in line for x in allow): continue
                pre=line[:m.end()]; hx=re.search(r"[0-9a-fA-F]+$",pre)
                kn=re.search(r"([\w.-]+)[\"\x27]?\s*[:=]\s*[\"\x27]?(?:@?sha(?:256|512):)?[0-9a-fA-F]+$",pre)
                name=kn.group(1) if kn else ""
                if name and re.search(CW,name,re.I): print(f"SECRET {f}:{n}"); bad=1; continue
                if hx and re.search(r"(sha256|sha512):$",pre[:hx.start()]): continue
                if name and re.search(r"(sha256|sha512|digest|checksum)$",name,re.I): continue
                print(f"SECRET {f}:{n}"); bad=1
        for m in re.finditer(r"[A-Za-z0-9+/_=-]{40,}",line):
            s=m.group(0)
            if re.fullmatch(r"[A-Z0-9_]+",s) or re.fullmatch(r"[0-9a-f]+",s) or any(x in s or x in line for x in allow): continue
            e=-sum(s.count(c)/len(s)*math.log2(s.count(c)/len(s)) for c in set(s))
            if e>4.5 and not s.startswith(("sha","http")): print(f"ENTROPY {f}:{n} {s[:12]}... H={e:.2f}"); bad=1
sys.exit(bad)')
}
# raw tree (before .publishignore), only venv/caches skipped; exact-path allowlist FORBID_ALLOW (add path + comment why)
FORBID_ALLOW=()
tracked_bad() {
  (cd "$ROOT" && find . \( -name .venv -o -name __pycache__ -o -name '.*_cache' -o -name .git \) -prune -o -type f -print | sed 's|^\./||' \
    | grep -E '(^|/)(\.env(\.[^/]*)?|known_hosts[^/]*|id_(ed25519|rsa)[^/]*|[^/]*\.(pem|key|keyfile)|fal\.key)$' || true) \
    | while IFS= read -r p; do for a in "${FORBID_ALLOW[@]+"${FORBID_ALLOW[@]}"}"; do [ "$p" = "$a" ] && continue 2; done; echo "$p"; done
}

if [ "$DRY" = 0 ]; then
  [ -n "$TF" ] && [ -f "$TF" ] || { echo "refused: --expect-token-file missing or not a file" >&2; exit 3; }
  [ -n "${HYPERTRAIN_PUBLISH_APPROVED:-}" ] && [ "$HYPERTRAIN_PUBLISH_APPROVED" = "$(cat "$TF")" ] || { echo "refused: HYPERTRAIN_PUBLISH_APPROVED does not match expected token" >&2; exit 3; }
fi
echo "== files to push =="
if [ -n "$OUT" ]; then mkdir -p "$OUT"; list_files > "$OUT/files.txt"; fi
list_files; echo "count: $(list_files | wc -l)"
for f in docs/assets/hypertrain-hero.png README.md LICENSE NOTICE CHANGELOG.md; do
  list_files | grep -qx "$f" || { echo "MISSING from push list: $f" >&2; exit 1; }; done

echo "== secret scan =="
B="$(tracked_bad)"; if [ -n "$B" ]; then echo "forbidden files (raw tree):" >&2; echo "$B" >&2; [ -n "$OUT" ] && echo "$B" > "$OUT/scan.txt"; exit 1; fi
if command -v gitleaks >/dev/null; then gitleaks detect --no-git --source "$ROOT" -q || { echo "gitleaks hit" >&2; exit 1; }; else echo "gitleaks not installed (skipped)"; fi
S="$(scan 2>&1 || true)"; if [ -n "$OUT" ]; then printf '%s\n' "${S:-clean}" > "$OUT/scan.txt"; fi
if [ -n "$S" ]; then echo "$S" >&2; echo "secret scan FAILED" >&2; exit 1; fi
echo "secret scan clean"

echo "== remote =="
if command -v gh >/dev/null; then
  gh api "repos/$REPO" --jq '"permissions: \(.permissions)"' 2>&1 || echo "gh: cannot read repo (non-fatal in dry-run)"
  gh api "repos/$REPO/commits?per_page=1" --jq 'length' 2>&1 | sed 's/^/commits on default branch (0 or error=empty): /' || true
  gh api user/packages -q length >/dev/null 2>&1 && echo "GHCR: packages API reachable" || echo "GHCR: write:packages scope not confirmed (gh auth status)"
else echo "gh missing"; fi
[ "$DRY" = 1 ] && { echo "dry-run OK"; exit 0; }

[ -n "$TF" ] && [ -f "$TF" ] || { echo "refused: --expect-token-file missing or not a file" >&2; exit 3; }
[ -n "${HYPERTRAIN_PUBLISH_APPROVED:-}" ] && [ "$HYPERTRAIN_PUBLISH_APPROVED" = "$(cat "$TF")" ] || { echo "refused: HYPERTRAIN_PUBLISH_APPROVED does not match expected token" >&2; exit 3; }
if [ "$TAG" = 1 ]; then
  TT="${HYPERTRAIN_TAG_TOKEN_FILE:-$TF.tag}"; [ -f "$TT" ] || { echo "refused: tag token file missing ($TT)" >&2; exit 3; }
  [ -n "${HYPERTRAIN_TAG_APPROVED:-}" ] && [ "$HYPERTRAIN_TAG_APPROVED" = "$(cat "$TT")" ] && [ "$HYPERTRAIN_TAG_APPROVED" != "$HYPERTRAIN_PUBLISH_APPROVED" ] || { echo "refused: tag token missing/identical/mismatch" >&2; exit 3; }
fi
R="$(gh api "repos/$REPO/commits?per_page=1" --jq length 2>&1 || true)"
if printf '%s' "$R" | grep -Eq '^[0-9]+$'; then N="$R"
elif printf '%s' "$R" | grep -q 'Git Repository is empty'; then N=0
else echo "refused: cannot determine emptiness of $REPO: $R" >&2; exit 6; fi
if [ "$N" != 0 ] && [ "$TAG" = 0 ]; then echo "refused: $REPO is not empty" >&2; exit 4; fi
TMP="$(mktemp -d /tmp/hypertrain-publish.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
case "$TMP" in "$ROOT"*) echo "temp inside workspace" >&2; exit 5;; esac
list_files | while IFS= read -r f; do mkdir -p "$TMP/$(dirname "$f")"; cp -p "$ROOT/$f" "$TMP/$f"; done
cd "$TMP"
if [ "$TAG" = 0 ]; then
  git init -q -b main; git add -A; git -c user.name="echobt" -c user.email="154886644+echobt@users.noreply.github.com" commit -qm "Hypertrain v0.1.0"
  git remote add origin "https://github.com/$REPO.git"; git push origin main
else
  git clone -q "https://github.com/$REPO.git" . ; git tag v0.1.0; git push origin v0.1.0
fi
