#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
    echo 'FlashNote Linux packaging requires an x86_64 Linux host.' >&2
    exit 1
fi

cargo build --release --locked --bin suojian --bin flashnote
bin_dir="${CARGO_TARGET_DIR:-target}/release"
version=$("$bin_dir/flashnote" --version)
version=${version#flashnote }
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
name="FlashNote-${version}-linux-x86_64"
stage=$(mktemp -d)
trap 'rm -f -- "$stage/$name/flashnote-desktop" "$stage/$name/flashnote" "$stage/$name/README.txt"; rmdir "$stage/$name" "$stage" 2>/dev/null || true' EXIT
mkdir -p runtime/packages "$stage/$name"
install -m 755 "$bin_dir/suojian" "$stage/$name/flashnote-desktop"
install -m 755 "$bin_dir/flashnote" "$stage/$name/flashnote"
cat > "$stage/$name/README.txt" <<'EOF'
FlashNote for Linux (x86_64, built on Ubuntu 24.04)

Run ./flashnote-desktop from a Wayland or X11 session.
Run ./flashnote --help to see the command-line interface.
The CLI opens the vault when the desktop application is closed.
The desktop and CLI use the session's Secret Service and an unlocked keyring.
On Ubuntu 24.04 the runtime libraries are available via:
  sudo apt install libasound2t64 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libvulkan1 mesa-vulkan-drivers
For transcription, configure paths to a local whisper-cli and GGML model.
EOF
tar -C "$stage" -czf "runtime/packages/$name.tar.gz" "$name"
printf '%s\n' "runtime/packages/$name.tar.gz"
