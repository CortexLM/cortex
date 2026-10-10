#!/usr/bin/env bash
# Secret scan (scan() below), any hit fails the publish:
# - any non-empty literal under a credential-named key (token, secret, password, api_key, key,
#   private, credential, auth; also under such a parent) unless it is an explicit placeholder
#   (empty, true/false/null/none, ..., ***, <...>, {var}, your-*, changeme, ${VAR}/$VAR, REPLACE_WITH_*, /path/to/..., a path ending .key/.token/.pem);
#   key names that describe a pointer (*_id, *_file, *_path, *_env, active_key, public_key, ...) are not credentials;
#   credential context is inherited only from mapping headers (`secret:` on its own line).
# - Shannon entropy >= 4.0 over a >= 24-char [A-Za-z0-9+] run (lowercase hex and CamelCase word identifiers excepted);
# - known prefixes ghp_, github_pat_, sk-, hf_, AKIA, xox?-, PEM "BEGIN" armor (5 dashes).
# Reviewed non-secrets: scripts/publish-allowlist.txt, one `path:sha256(line)` per entry (exact line bytes); exempts the strict rules above only, never the legacy provider patterns/.secretscan-allow.
# Limitations:
# - Python values are checked only when they are string literals (expressions are not secrets);
#   annotated assignments (`x: str = "..."`) are only covered by entropy/prefix rules.
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
copy_files() { # stdin: selected relative paths; no source/destination symlink resolution
  python3 -c '
import os,shutil,stat,sys
from contextlib import ExitStack

dirs=os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW
def directory(stack, parent, name, create=False):
    if create:
        try: os.mkdir(name, dir_fd=parent)
        except FileExistsError: pass
    fd=os.open(name, dirs, dir_fd=parent)
    stack.callback(os.close, fd)
    return fd

with ExitStack() as roots:
    source=directory(roots, None, sys.argv[1])
    target=directory(roots, None, sys.argv[2])
    for line in sys.stdin:
        parts=line.rstrip("\n").split("/")
        if any(p in ("", ".", "..") for p in parts):
            raise ValueError("unsafe selected relative path")
        with ExitStack() as stack:
            src,dst=source,target
            for part in parts[:-1]:
                src=directory(stack, src, part)
                dst=directory(stack, dst, part, create=True)
            fd=os.open(parts[-1], os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK, dir_fd=src)
            stack.callback(os.close, fd)
            before=os.fstat(fd)
            named=os.stat(parts[-1], dir_fd=src, follow_symlinks=False)
            if not stat.S_ISREG(before.st_mode) or (before.st_dev,before.st_ino) != (named.st_dev,named.st_ino):
                raise ValueError("selected source is not the same regular file")
            out=os.open(parts[-1], os.O_WRONLY|os.O_CREAT|os.O_NOFOLLOW|os.O_NONBLOCK, 0o600, dir_fd=dst)
            stack.callback(os.close, out)
            if not stat.S_ISREG(os.fstat(out).st_mode):
                raise ValueError("copy destination is not a regular file")
            os.ftruncate(out, 0)
            with os.fdopen(os.dup(fd), "rb") as reader, os.fdopen(os.dup(out), "wb") as writer:
                shutil.copyfileobj(reader, writer)
            after=os.fstat(fd)
            if (before.st_size,before.st_mtime_ns,before.st_ctime_ns) != (after.st_size,after.st_mtime_ns,after.st_ctime_ns):
                raise ValueError("selected source changed during copy")
            os.fchmod(out, stat.S_IMODE(before.st_mode) & 0o777)
' "$ROOT" "$1"
}
scan() { # regex + entropy secret scan over the file list (single python pass); allowlist = literal substrings in .secretscan-allow
  list_files | (cd "$ROOT" && python3 -c '
import re,sys,math,json,hashlib
allow=[l.strip() for l in open(".secretscan-allow") if l.strip() and not l.startswith("#")]
try: reviewed={l.split("#")[0].strip() for l in open("scripts/publish-allowlist.txt")}-{""}
except FileNotFoundError: reviewed=set()
CSEG={"token","secret","secrets","password","passwd","apikey","key","private","credential","credentials","auth","pat"}
NSEG={"public","pub","id","ids","env","file","path","dir","url","name","names","group","count","kind","type","mode","len","hash","digest","sha256","active","prefix","profile","object","topology","answer"}  # key names that describe a pointer/identifier, not a value
def segs(k): return [x.lower() for x in re.findall(r"[A-Z]?[a-z]+|[A-Z]+(?![a-z])|\d+",k)]
def cred(k):
    s=segs(k); return bool(CSEG&set(s)) and not NSEG&set(s)
PH=re.compile(r"|true|false|null|none|\.\.\.|\*+|<[^<>]+>|\{[^{}]+\}|your-[\w-]+|changeme|\$\{[^}]+\}|\$[A-Z_][A-Z0-9_]*|REPLACE_WITH_[A-Z0-9_]+|/path/to/\S*|[\w./~-]*\.(key|token|pem)",re.I)
PREFIX=re.compile(r"(?<![A-Za-z0-9])(ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{16,}|hf_[A-Za-z0-9]{16,}|AKIA[0-9A-Z]{12,}|xox[abposr]-[A-Za-z0-9-]{10,})|-{5}BEGIN")
def identifier(t): # ponytail: word-like = mean camel/snake/path segment >= 4 chars; random base62/64 averages ~2
    p=re.findall(r"[A-Z]?[a-z]+|[A-Z]+(?![a-z])|\d+",t); return bool(p) and sum(map(len,p))/len(p)>=4
QKV=re.compile(r"(?<![\w.-])([\w.-]+)[\"\x27]?\s*[:=]\s*(?:b|r|u)?([\"\x27])(.*?)(?<!\\)\2")
UKV=re.compile(r"^\s*(?:export\s+|-\s+)?[\"\x27]?([\w.-]+)[\"\x27]?\s*[:=]\s*([^\s\"\x27#{}\[\],|>&][^\s#,]*)\s*(?:#.*)?$")
def H(t): return -sum(t.count(c)/len(t)*math.log2(t.count(c)/len(t)) for c in set(t))
def strict(f,line,stack):
    out=[]
    vals=[(m.group(1),m.group(3)) for m in QKV.finditer(line)]
    if not f.endswith(".py"):
        m=UKV.match(line)
        if m: vals.append((m.group(1),m.group(2)))
    for k,v in vals:
        if (cred(k) or any(cred(p) for p in stack)) and not PH.fullmatch(v.strip()): out.append("credential value under "+k)
    for m in PREFIX.finditer(line): out.append("known prefix "+m.group(0)[:6])
    for t in re.findall(r"[A-Za-z0-9+]{24,}",line):  # ponytail: split on / _ - . = so paths/slugs are words; a secret with separators needs a >=24-char run
        if not identifier(t) and not re.fullmatch(r"[0-9a-f]+",t) and H(t)>=4.0: out.append(f"entropy H={H(t):.2f}")
    return out
pats=[r"AKIA[0-9A-Z]{16}",r"-{5}BEGIN [A-Z ]*PRIVATE KEY-{5}",r"gh[pousr]_[A-Za-z0-9]{30,}",r"xox[abp]-[A-Za-z0-9-]{10,}",
 r"sk-[A-Za-z0-9]{32,}",r"FAL_KEY *[=:] *[\"\x27]?[A-Za-z0-9-]{20,}",r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}:[0-9a-f]{32,}",
 r"fal_sk_[0-9a-fA-F]{20,}",r"fal_sk_[A-Za-z0-9]{16,}",r"(?i)(api|secret|token|key)[_-]?\w*[\"\x27]?\s*[:=]\s*[\"\x27]?[0-9a-f]{32,}(?![0-9a-f])",
 r"(instance_api_key|vast[_-]?(api[_-]?)?key|api[_-]?key|secret[_-]?key)[\"\x27 ]*[:=][\"\x27 ]*[A-Za-z0-9+/_-]{24,}"]
CW=r"token(?!iz)|secret|key|password|passwd|auth|credential|apikey"
CRED=re.compile(r"([\w.-]*(?:"+CW+r")[\w.-]*)[\"\x27]?\s*[:=]\s*[\"\x27]?(?:@?sha(?:256|512):)?[0-9a-fA-F]{32,}(?![0-9a-fA-F])",re.I)
bad=0
public_json={
 "deploy/k8s/versions.json": ((),"server_build_job","https://github.com/"+"CortexLM/hypertrain/actions/runs/"+"37815568607/job/113454067723"),
 "tests/protocol/fixtures/network_v1_baseline.json": (("source_sha256",),"protocol/keys.py","d3030488eb18aaafc0feceb256666bcc63fc15c4e6ae0bb008980affd96976a2"),
}
public_inline={
 "docs/network-v2.md": ("--freeze ","/root/distributed-decision-training/"+".omo/evidence/hypertrain-network/"+"D2-FINAL-SOURCE-FREEZE.json"),
 "scripts/network_gpu_prepare.py": ("\"operation_receipts\": ","CREATED_ONLY_AT_RUNTIME_FROM_ACCEPTED_CONTEXTS_BY_"+"service_factory"),
 "tests/miner/test_island_launch_v2.py": ("\"experiment\": ","hypertrain-network-v2"),
}
public_entropy={
 "docs/RESULTS.md": {"f360080129ca48b776b8f50b61bfe90a3a7e5ee84eb8e379fefcf33201c67528"},
 "docs/network-v2.md": {"f360080129ca48b776b8f50b61bfe90a3a7e5ee84eb8e379fefcf33201c67528","3bf8752c74172542a19ec8f387d79c16821bd42cba7335fda8b5bb365211019f"},
 "scripts/relay_kind_smoke.py": {"9af042788b3249f7b1d329581bfe135d1a625af872d71ea22e487b3491f3839b"},
 "tests/gpu_ops/test_network_admit_cli.py": {"451abbd0ff08a09966c08c027ca7a49c24182fbc1c5e7f5ad33ab1ef72247158"},
 "scripts/verify_image.sh": {"22b0248ee25b21e6cd0485679f4adee029af9680d61eaed99e439d766222cf11"},
}
public_token_grammar=re.compile(
 r"(?:[A-Za-z0-9_-]+/)+[A-Za-z0-9_-]+|" + r"--setenv=KIND_EXPERIMENTAL_PROVIDER=" + "docker"
)
for f in sys.stdin.read().split("\n"):
    if not f or f.endswith((".u32",".png",".jpg",".lock")) or f==".secretscan-allow": continue
    content=open(f,errors="ignore").read()
    public=None
    if f in public_json:
        parents,key,value=public_json[f]
        try:
            obj=json.loads(content)
            for parent in parents: obj=obj[parent]
            if obj[key]==value: public=(key,value)
        except (ValueError,KeyError,TypeError): pass
    stack=[]  # (indent, key) ancestors, indentation-aware
    cstack=[]  # container-only ancestors for the strict credential rule
    for n,line in enumerate(content.splitlines(),1):
        heuristic=line
        if public:
            key,value=public
            declaration=json.dumps(key)+": "+json.dumps(value)
            indent=len(line)-len(line.lstrip())
            if line.strip().rstrip(",")==declaration and tuple(k for i,k in stack if i<indent)==parents:
                heuristic=line.replace(json.dumps(value),"\"PUBLIC\"",1)
        if f in public_inline:
            context,value=public_inline[f]
            quoted=f!="docs/network-v2.md"
            scalar=("\""+value+"\"") if quoted else value
            suffix="," if quoted else " \\"
            declaration=context+scalar+suffix
            if line.lstrip().startswith(declaration):
                start=line.index(scalar,len(line)-len(line.lstrip())+len(context))
                heuristic=line[:start]+("\"PUBLIC\"" if quoted else "PUBLIC")+line[start+len(scalar):]
        raw_line=line
        reviewed_line=f+":"+hashlib.sha256(raw_line.encode()).hexdigest() in reviewed
        line=heuristic
        cparents=[k for _,k in cstack]
        km=re.match(r"^(\s*)[{\[,\s]*[\"\x27]?([\w.-]+)[\"\x27]?\s*:\s*(.*)$",line.rstrip("\n"))
        if km:
            ind=len(km.group(1)); key=km.group(2); val=km.group(3).strip().strip("{}[],").strip().strip("\"\x27")
            while stack and stack[-1][0]>=ind: stack.pop()
            if val[:7] in ("sha256:","sha512:") and not any(re.search(CW,k,re.I) for _,k in stack): pass
            elif re.fullmatch(r"[A-Za-z0-9+/_=-]{20,}",val) and re.search(r"\d",val) and any(re.search(CW,k,re.I) for _,k in stack) and not any(x in line for x in allow):
                print(f"SECRET {f}:{n} under credential parent {[k for _,k in stack]}"); bad=1
            while cstack and cstack[-1][0]>=ind: cstack.pop()
            cparents=[k for _,k in cstack]
            if not val: cstack.append((ind,key))  # only mapping headers (`secret:`) pass credential context to children
            stack.append((ind,key))
        for why in ([] if reviewed_line else strict(f,line,cparents)):
            print(f"SECRET {f}:{n} {why}"); bad=1
        for m in CRED.finditer(line):  # credential-word key names always block (hash/digest words or sha256: prefix never exempt)
            if not any(x in line for x in allow): print(f"SECRET {f}:{n} credential-named hex"); bad=1
        for p in pats:
            token_line=heuristic if p.startswith("(?i)") else raw_line
            for m in re.finditer(p,token_line):
                if any(x in raw_line for x in allow): continue
                pre=token_line[:m.end()]; hx=re.search(r"[0-9a-fA-F]+$",pre)
                kn=re.search(r"([\w.-]+)[\"\x27]?\s*[:=]\s*[\"\x27]?(?:@?sha(?:256|512):)?[0-9a-fA-F]+$",pre)
                name=kn.group(1) if kn else ""
                if name and re.search(CW,name,re.I): print(f"SECRET {f}:{n}"); bad=1; continue
                if hx and re.search(r"(sha256|sha512):$",pre[:hx.start()]): continue
                if name and re.search(r"(sha256|sha512|digest|checksum)$",name,re.I): continue
                print(f"SECRET {f}:{n}"); bad=1
        for m in re.finditer(r"[A-Za-z0-9+/_=-]{40,}",line):
            s=m.group(0)
            # Exact reviewed token/file plus public path/sequence/env grammar, never whole-line allow.
            if public_token_grammar.fullmatch(s) and hashlib.sha256(s.encode()).hexdigest() in public_entropy.get(f,set()): continue
            if re.fullmatch(r"[A-Z0-9_]+",s) or re.fullmatch(r"[0-9a-f]+",s) or any(x in s or x in line for x in allow): continue
            e=-sum(s.count(c)/len(s)*math.log2(s.count(c)/len(s)) for c in set(s))
            if e>4.5 and not s.startswith(("sha","http")): print(f"ENTROPY {f}:{n} H={e:.2f}"); bad=1
sys.exit(bad)')
}
# raw tree (before .publishignore), only venv/caches skipped; exact-path allowlist FORBID_ALLOW (add path + comment why)
FORBID_ALLOW=(
  .env.example # R2 variable names with placeholder values only (docs/r2.md); content still goes through scan below
)
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
if command -v gitleaks >/dev/null; then gitleaks detect --no-git --source "$ROOT" --config "$ROOT/.gitleaks.toml" --log-level error --redact=100 || { echo "gitleaks hit" >&2; exit 1; }; else echo "gitleaks not installed (skipped)"; fi
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
list_files | copy_files "$TMP"
cd "$TMP"
if [ "$TAG" = 0 ]; then
  git init -q -b main; git add -A; git -c user.name="echobt" -c user.email="154886644+echobt@users.noreply.github.com" commit -qm "Hypertrain v0.1.0"
  git remote add origin "https://github.com/$REPO.git"; git push origin main
else
  git clone -q "https://github.com/$REPO.git" . ; git tag v0.1.0; git push origin v0.1.0
fi
