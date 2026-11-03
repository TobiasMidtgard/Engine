#!/bin/sh
# Downloads the wiki's two fonts (Inter for text, Cormorant Garamond for titles, both SIL OFL 1.1)
# from npm into src/fonts/, with their license files. They stay out of git (AGENTS.md: no
# third-party files in the repository); the Wiki workflow runs this before building. Without
# them the wiki falls back to system fonts, so a local preview works without this step.
set -eu
cd "$(dirname "$0")/.."
mkdir -p src/fonts
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() { # npm package, version, sha256 of the package, font files to keep
    package=$1 version=$2 sum=$3
    shift 3
    curl -fsSL -o "$tmp/$package.tgz" \
        "https://registry.npmjs.org/@fontsource-variable/$package/-/$package-$version.tgz"
    echo "$sum  $tmp/$package.tgz" | sha256sum -c - >/dev/null
    mkdir -p "$tmp/$package"
    tar xzf "$tmp/$package.tgz" -C "$tmp/$package"
    for file in "$@"; do
        cp "$tmp/$package/package/files/$file" src/fonts/
    done
    cp "$tmp/$package/package/LICENSE" "src/fonts/$package-OFL.txt"
}

fetch inter 5.3.0 d26710dd38e7217484a1d47ee76c024977ec550548876f26d06ed1844af09a14 \
    inter-latin-wght-normal.woff2 inter-latin-wght-italic.woff2
fetch cormorant-garamond 5.3.0 81d1d307be5bf24c09112189efa402e29ad3c7772db8d308c0c6f36f327199cf \
    cormorant-garamond-latin-wght-normal.woff2 cormorant-garamond-latin-wght-italic.woff2
echo "fonts: $(ls src/fonts | wc -l) files in wiki/src/fonts"
