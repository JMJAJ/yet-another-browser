#!/usr/bin/env bash
# Build a release and lay it out in dist/brxswer/ with the engine files it
# needs (one locale), then zip it to dist/brxswer-<version>-windows-x64.zip.
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/fetch-cef.sh
NINJA="/c/Program Files/Microsoft Visual Studio/2022/Enterprise/Common7/IDE/CommonExtensions/Microsoft/CMake/Ninja"
[ -d "$NINJA" ] && export PATH="$NINJA:$PATH"
cargo build --release
VER=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT=dist/brxswer
rm -rf "$OUT" && mkdir -p "$OUT/locales"
cp target/release/brxswer.exe LICENSE "$OUT/"
for f in libcef.dll chrome_elf.dll d3dcompiler_47.dll dxcompiler.dll vk_swiftshader.dll vk_swiftshader_icd.json \
         vulkan-1.dll icudtl.dat v8_context_snapshot.bin resources.pak chrome_100_percent.pak chrome_200_percent.pak; do
  cp ".cef/$f" "$OUT/"
done
cp .cef/locales/en-US.pak "$OUT/locales/"
cp .cef/CREDITS.html "$OUT/chromium-credits.html"
ZIP="dist/brxswer-$VER-windows-x64.zip"
rm -f "$ZIP"
powershell -NoProfile -Command "Compress-Archive -Path '$OUT' -DestinationPath '$ZIP' -CompressionLevel Optimal"
du -sh "$OUT" "$ZIP"
