#!/bin/sh
# Packs the release binaries into todai-VERSION-x86_64-linux-gnu.tar.gz plus
# its .sha256, in OUTDIR. Run after `cargo build --release --locked`.
# Used by .github/workflows/release.yml; works the same by hand.
set -eu
outdir=${1:-dist}
version=$(cargo metadata --no-deps --format-version 1 --locked |
    sed -n 's/.*"name":"todai","version":"\([^"]*\)".*/\1/p')
[ -n "$version" ] || { echo "package.sh: can't read todai's version" >&2; exit 1; }
name="todai-$version-x86_64-linux-gnu"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
for bin in todai todai-server todai-tui; do
    install -m755 "target/release/$bin" "$stage/$name/"
done
install -m644 README.md LICENSE "$stage/$name/"
mkdir -p "$outdir"
tar -C "$stage" -czf "$outdir/$name.tar.gz" "$name"
(cd "$outdir" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$outdir/$name.tar.gz"
