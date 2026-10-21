#!/usr/bin/env bash
# Recreate packaging/icons/*x*/gluco.png from the text file next to this script.
# A checkout that only has text files still gets the menu icon.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

rel=""
while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ "$line" == *.png ]]; then
        rel="$line"
        continue
    fi
    if [[ -z "$rel" || -z "$line" ]]; then
        continue
    fi
    mkdir -p "$(dirname "$rel")"
    printf '%s\n' "$line" | base64 -d > "$rel"
    rel=""
done < gluco-icons.b64
