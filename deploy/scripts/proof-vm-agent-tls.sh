#!/usr/bin/env bash
# Mint the proof-vm-orchestrator agent certificate with the SANs the control
# plane actually connects to — on the KVM host, with openssl only.
#
# The CP's rustls client (proof-vm-fc) refuses a certificate that has no SAN
# for the host in PROOF_VM_ORCHESTRATOR_URL (a CN is never a name), and the
# agent itself now refuses to boot on such a certificate (PROOF_VM_AGENT_TLS_SANS).
# This script collects every name that URL may use for this host and puts all
# of them on one certificate signed by a private CA kept on this host:
#
#   --san NAME|IP          explicit (repeat); the dedicated droplet's VPC IP, a DNS name, …
#   PROOF_VM_AGENT_TLS_SANS  from --env-file (comma-separated), same meaning
#   PROOF_VM_AGENT_BIND      from --env-file: its host when it is a specific address
#   hostname -f            unless --no-hostname (or --hostname NAME to override)
#   VPC address            the first IPv4 on --vpc-iface (default eth1, DO's VPC NIC) unless --no-vpc
#
# Production is a DEDICATED droplet on the VPC: the CP reaches it on the VPC
# IP (and/or a hostname), so those are the SANs. A docker gateway address
# (172.17–172.31.x) only serves a CP colocated on the same box — a staging
# artefact; the script warns when that is all it was given.
#
# Usage:
#   proof-vm-agent-tls.sh [--out-dir /etc/proof-vm] [--env-file OUT_DIR/orchestrator.env]
#                         [--san NAME|IP]... [--hostname NAME | --no-hostname]
#                         [--vpc-iface IFACE | --no-vpc] [--days 825] [--ca-days 3650]
#                         [--force] [--write-env] [--dry-run]
#
# Files (OUT_DIR): ca.key 0400 (never leaves this host) · ca.pem 0644 (copy to the
# CP as deploy/secrets/proof/vm_orchestrator_ca.pem) · tls.key 0400 · tls.crt 0644.
# An existing CA is reused (the CP keeps trusting it); --force re-mints it too.
# --write-env sets PROOF_VM_AGENT_TLS_SANS in the env file to exactly the SANs minted.
#
# Exit: 0 minted (or --dry-run listed), 1 refused / failed.
set -euo pipefail

RED() { printf '\033[31m%s\033[0m\n' "$*" >&2; }
YEL() { printf '\033[33m%s\033[0m\n' "$*" >&2; }
LOG() { printf '[agent-tls] %s\n' "$*"; }
die() { RED "refusing: $*"; exit 1; }

OUT_DIR="/etc/proof-vm"
ENV_FILE=""
SANS=()
HOSTNAME_OVERRIDE=""
USE_HOSTNAME=1
VPC_IFACE="eth1"
USE_VPC=1
DAYS=825
CA_DAYS=3650
FORCE=0
WRITE_ENV=0
DRY_RUN=0

usage() { sed -n '2,33p' "$0"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir) OUT_DIR="${2:?}"; shift 2 ;;
    --env-file) ENV_FILE="${2:?}"; shift 2 ;;
    --san) SANS+=("${2:?}"); shift 2 ;;
    --hostname) HOSTNAME_OVERRIDE="${2:?}"; shift 2 ;;
    --no-hostname) USE_HOSTNAME=0; shift ;;
    --vpc-iface) VPC_IFACE="${2:?}"; shift 2 ;;
    --no-vpc) USE_VPC=0; shift ;;
    --days) DAYS="${2:?}"; shift 2 ;;
    --ca-days) CA_DAYS="${2:?}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --write-env) WRITE_ENV=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) RED "unknown arg: $1"; usage; exit 1 ;;
  esac
done
[[ -n "$ENV_FILE" ]] || ENV_FILE="$OUT_DIR/orchestrator.env"
command -v openssl >/dev/null 2>&1 || die "openssl not found on PATH"

# Value from the env file (last KEY= wins; comments ignored; quotes stripped).
cfg() {
  local name="$1" line val=""
  if [[ -f "$ENV_FILE" ]]; then
    line="$(grep -E "^[[:space:]]*(export[[:space:]]+)?${name}=" "$ENV_FILE" | tail -n1 || true)"
    val="${line#*=}"
    val="${val%\"}"; val="${val#\"}"; val="${val%\'}"; val="${val#\'}"
  fi
  printf '%s' "$val"
}

is_ipv4() { [[ "$1" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; }
is_ipv6() { [[ "$1" == *:* ]] && [[ "$1" =~ ^[0-9a-fA-F:.]+$ ]]; }
is_dns()  { [[ "$1" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$ ]]; }
# 172.17.0.0–172.31.255.255: docker's default bridge + compose networks.
is_docker_bridge() {
  [[ "$1" =~ ^172\.([0-9]+)\. ]] || return 1
  local o="${BASH_REMATCH[1]}"
  (( o >= 17 && o <= 31 ))
}

# host part of an ADDR:PORT bind ([v6]:port, v4:port, or a bare host)
bind_host() {
  local b="$1"
  if [[ "$b" == \[* ]]; then printf '%s' "${b#\[}" | cut -d']' -f1; else printf '%s' "${b%:*}"; fi
}

add_san() { # add_san VALUE SOURCE
  local v s="$2"
  v="$(printf '%s' "$1" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
  [[ -n "$v" ]] || return 0
  if is_ipv4 "$v" || is_ipv6 "$v"; then :; elif is_dns "$v"; then :; else die "SAN '$v' ($s) is neither an IP nor a DNS name"; fi
  local x
  for x in "${ALL[@]:-}"; do [[ "$x" == "$v" ]] && return 0; done
  ALL+=("$v"); SRC+=("$s")
}

ALL=(); SRC=()
for s in "${SANS[@]:-}"; do [[ -n "$s" ]] && add_san "$s" "--san"; done
env_sans="$(cfg PROOF_VM_AGENT_TLS_SANS)"
if [[ -n "$env_sans" ]]; then
  while IFS= read -r s; do add_san "$s" "PROOF_VM_AGENT_TLS_SANS ($ENV_FILE)"; done < <(printf '%s\n' "$env_sans" | tr ',' '\n')
fi
bind="$(cfg PROOF_VM_AGENT_BIND)"
if [[ -n "$bind" ]]; then
  bh="$(bind_host "$bind")"
  case "$bh" in
    0.0.0.0|::|"") LOG "PROOF_VM_AGENT_BIND=$bind is a wildcard: it names no host; the SANs must come from the other sources" ;;
    *) add_san "$bh" "PROOF_VM_AGENT_BIND ($ENV_FILE)" ;;
  esac
fi
if [[ -n "$HOSTNAME_OVERRIDE" ]]; then
  add_san "$HOSTNAME_OVERRIDE" "--hostname"
elif [[ "$USE_HOSTNAME" -eq 1 ]]; then
  fq="$(hostname -f 2>/dev/null || hostname 2>/dev/null || true)"
  fq="$(printf '%s' "$fq" | tr '[:upper:]' '[:lower:]')"
  if [[ -n "$fq" && "$fq" != "localhost" && "$fq" != *.localdomain ]] && is_dns "$fq"; then
    add_san "$fq" "hostname -f"
  else
    LOG "hostname -f gave '${fq:-<empty>}'; not usable as a SAN (pass --hostname NAME for a DNS name the CP resolves)"
  fi
fi
if [[ "$USE_VPC" -eq 1 ]]; then
  if command -v ip >/dev/null 2>&1 && ip -4 -o addr show dev "$VPC_IFACE" >/dev/null 2>&1; then
    vpc="$(ip -4 -o addr show dev "$VPC_IFACE" | awk '{print $4}' | cut -d/ -f1 | head -n1)"
    [[ -n "$vpc" ]] && add_san "$vpc" "VPC address on $VPC_IFACE"
  else
    LOG "no IPv4 on $VPC_IFACE (pass --vpc-iface IFACE, or --no-vpc when the CP reaches this host another way)"
  fi
fi

[[ ${#ALL[@]} -gt 0 ]] || die "no SAN: pass --san <VPC IP or hostname the CP's PROOF_VM_ORCHESTRATOR_URL uses> (or set PROOF_VM_AGENT_TLS_SANS / a specific PROOF_VM_AGENT_BIND in $ENV_FILE)"

only_bridge=1
alt=""
for i in "${!ALL[@]}"; do
  v="${ALL[$i]}"
  if is_ipv4 "$v" || is_ipv6 "$v"; then alt+="${alt:+,}IP:$v"; else alt+="${alt:+,}DNS:$v"; fi
  is_ipv4 "$v" && is_docker_bridge "$v" || only_bridge=0
  LOG "SAN $v  (${SRC[$i]})"
done
if [[ "$only_bridge" -eq 1 ]]; then
  YEL "WARN  every SAN is a docker bridge address (172.17–31.x): that only serves a CP colocated on this box (staging). A production CP reaches the DEDICATED droplet over the VPC — add --san <vpc-ip> / --san <hostname>."
fi
sans_csv="$(IFS=,; printf '%s' "${ALL[*]}")"

if [[ "$DRY_RUN" -eq 1 ]]; then
  LOG "dry run: subjectAltName=$alt"
  LOG "env line: PROOF_VM_AGENT_TLS_SANS=$sans_csv"
  exit 0
fi

umask 077
install -d -m 0750 "$OUT_DIR"
ca_key="$OUT_DIR/ca.key"; ca_pem="$OUT_DIR/ca.pem"
leaf_key="$OUT_DIR/tls.key"; leaf_crt="$OUT_DIR/tls.crt"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

if [[ "$FORCE" -eq 1 || ! -s "$ca_key" || ! -s "$ca_pem" ]]; then
  [[ "$FORCE" -eq 1 && -s "$ca_pem" ]] && YEL "WARN  --force re-mints the CA: the CP's vm_orchestrator_ca.pem must be replaced too"
  openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/ca.key"
  openssl req -x509 -new -key "$tmp/ca.key" -sha256 -days "$CA_DAYS" \
    -subj "/CN=proof-vm-orchestrator CA ${ALL[0]}" \
    -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -out "$tmp/ca.pem"
  install -m 0400 "$tmp/ca.key" "$ca_key"
  install -m 0644 "$tmp/ca.pem" "$ca_pem"
  LOG "CA minted: $ca_pem (key $ca_key stays on this host)"
else
  LOG "CA reused: $ca_pem"
fi

openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/tls.key"
openssl req -new -key "$tmp/tls.key" -sha256 -subj "/CN=proof-vm-orchestrator ${ALL[0]}" -out "$tmp/tls.csr"
cat > "$tmp/leaf.ext" <<EOF
basicConstraints=CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=$alt
EOF
openssl x509 -req -in "$tmp/tls.csr" -CA "$ca_pem" -CAkey "$ca_key" -CAcreateserial -CAserial "$tmp/ca.srl" \
  -sha256 -days "$DAYS" -extfile "$tmp/leaf.ext" -out "$tmp/tls.crt" 2>/dev/null
openssl verify -CAfile "$ca_pem" "$tmp/tls.crt" >/dev/null
install -m 0400 "$tmp/tls.key" "$leaf_key"
install -m 0644 "$tmp/tls.crt" "$leaf_crt"
LOG "agent certificate minted: $leaf_crt (key $leaf_key), $DAYS days"
openssl x509 -in "$leaf_crt" -noout -ext subjectAltName | sed 's/^/[agent-tls] /'

if [[ "$WRITE_ENV" -eq 1 ]]; then
  [[ -f "$ENV_FILE" ]] || die "--write-env: $ENV_FILE does not exist"
  if grep -qE '^[[:space:]]*(export[[:space:]]+)?PROOF_VM_AGENT_TLS_SANS=' "$ENV_FILE"; then
    sed -i -E "s|^([[:space:]]*(export[[:space:]]+)?PROOF_VM_AGENT_TLS_SANS=).*|\1$sans_csv|" "$ENV_FILE"
  else
    printf '\n# Hosts PROOF_VM_ORCHESTRATOR_URL may name for this agent; the certificate is checked against them at boot.\nPROOF_VM_AGENT_TLS_SANS=%s\n' "$sans_csv" >> "$ENV_FILE"
  fi
  LOG "$ENV_FILE: PROOF_VM_AGENT_TLS_SANS=$sans_csv"
else
  LOG "set in $ENV_FILE:  PROOF_VM_AGENT_TLS_SANS=$sans_csv   (or re-run with --write-env)"
fi
cat <<EOF
[agent-tls] next:
[agent-tls]   PROOF_VM_AGENT_TLS_CERT=$leaf_crt  PROOF_VM_AGENT_TLS_KEY=$leaf_key   (in $ENV_FILE)
[agent-tls]   systemctl restart proof-vm-orchestrator && journalctl -u proof-vm-orchestrator -n 5
[agent-tls]     → "listening (https); certificate covers every listed name" (a missing SAN exits 1 naming it)
[agent-tls]   copy $ca_pem to the CP: deploy/secrets/proof/vm_orchestrator_ca.pem (0400, uid 65532) →
[agent-tls]     PROOF_VM_ORCHESTRATOR_CA_FILE=/run/base/proof/vm_orchestrator_ca.pem; PROOF_VM_ORCHESTRATOR_URL=https://<one of: $sans_csv>:8200
[agent-tls]   then on the CP: deploy/scripts/proof-vm-wire-check.sh agent && deploy/scripts/proof-vm-wire-check.sh cp
EOF
