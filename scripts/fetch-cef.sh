#!/usr/bin/env bash
# Fetch the CEF build the `cef` crate expects into ./.cef (bundled-Chromium
# engine, `--features engine-cef`). The crate's own downloader currently
# fails on CEF's large index file, so this does the same job by hand:
# download the "minimal" Windows archive, check its SHA-1 against the
# official index, and lay it out the way cef-dll-sys looks for it.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="156.0.26+g8ea0c73+chromium-156.0.8078.88"
NAME="cef_binary_${VERSION}_windows64_minimal"
CDN="https://cef-builds.spotifycdn.com"

if [ -f .cef/archive.json ]; then
  echo ".cef already present"; exit 0
fi

mkdir -p .cef-dl
SHA=$(curl -s "$CDN/index.json" | python -c "
import sys, json
for v in json.load(sys.stdin)['windows64']['versions']:
    for f in v['files']:
        if f['name'] == '$NAME.tar.bz2':
            print(f['sha1'])
")
echo "downloading $NAME.tar.bz2 (about 175 MB)"
curl -L -o ".cef-dl/$NAME.tar.bz2" "$CDN/$(python -c "import urllib.parse; print(urllib.parse.quote('$NAME.tar.bz2'))")"
echo "$SHA  .cef-dl/$NAME.tar.bz2" | sha1sum -c -

rm -rf .cef-dl/x .cef && mkdir -p .cef-dl/x .cef
tar -xjf ".cef-dl/$NAME.tar.bz2" -C .cef-dl/x
X=".cef-dl/x/$NAME"
mv "$X"/Release/* .cef/
mv "$X"/Resources/* .cef/
mv "$X"/CMakeLists.txt "$X"/cmake "$X"/include "$X"/libcef_dll "$X"/CREDITS.html .cef/
printf '{\n  "type": "minimal",\n  "name": "%s.tar.bz2",\n  "sha1": "%s"\n}\n' "$NAME" "$SHA" > .cef/archive.json
rm -rf .cef-dl/x
echo "CEF ready in .cef"
