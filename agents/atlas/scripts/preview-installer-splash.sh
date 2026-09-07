#!/bin/sh

set -eu

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
repo_root=$(CDPATH= cd "$script_dir/.." && pwd)
installer="$repo_root/install.sh"
preview_dir=

cleanup() {
	if [ -n "${preview_dir:-}" ] && [ -d "$preview_dir" ]; then
		rm -rf "$preview_dir"
	fi
}

trap cleanup EXIT

if [ ! -f "$installer" ]; then
	printf 'error: installer script not found at %s\n' "$installer" >&2
	exit 1
fi

frames="${1:-${PRIME_AGENT_SPLASH_PREVIEW_FRAMES:-300}}"
case "$frames" in
	""|*[!0-9]*)
		printf 'error: frame count must be a positive integer.\n' >&2
		exit 1
		;;
	0)
		printf 'error: frame count must be greater than zero.\n' >&2
		exit 1
		;;
esac

preview_dir=$(mktemp -d "${TMPDIR:-/tmp}/prime-agent-splash-preview.XXXXXX")
preview_file="$preview_dir/preview.sh"

sed '/^main "\$@"$/,$d' "$installer" >"$preview_file"
cat >>"$preview_file" <<'EOF'

prime_agent_screen_enabled=1

preview_restore_terminal() {
	prime_agent_restore_terminal
}

preview_signal_cleanup() {
	prime_agent_restore_terminal
	exit "$1"
}

trap preview_restore_terminal EXIT
trap 'preview_signal_cleanup 130' INT
trap 'preview_signal_cleanup 143' TERM

i=0
while [ "$i" -lt "$1" ]; do
	prime_agent_screen "Installing Prime Agent" "Downloading Prime Agent$(prime_agent_pulse)" "Fetching the verified package." ""
	sleep 0.18
	i=$((i + 1))
done
EOF

sh "$preview_file" "$frames"
