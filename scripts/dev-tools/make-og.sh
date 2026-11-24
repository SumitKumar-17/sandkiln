#!/usr/bin/env bash
# Renders website/public/og.png (the 1200x630 social card) from the mark and the
# page headline, using headless Chrome. Needs google-chrome (the dev box has it).
#
# Usage: scripts/dev-tools/make-og.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
CHROME="${CHROME:-google-chrome}"
command -v "$CHROME" >/dev/null || { echo "make-og.sh: $CHROME not found (set CHROME=...)" >&2; exit 1; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
cp website/public/mark.svg "$TMP/mark.svg"
cat > "$TMP/og.html" <<'HTML'
<!doctype html><meta charset="utf-8">
<style>
  html,body{margin:0;background:#151515}
  body{width:1200px;height:630px;box-sizing:border-box;padding:0 110px;display:flex;flex-direction:column;justify-content:center;gap:40px;color:#ece9e3;font-family:Geist,"Helvetica Neue",Arial,sans-serif}
  .brand{display:flex;align-items:center;gap:22px;font-size:46px;font-weight:500;letter-spacing:-.02em}
  .brand img{width:64px;height:64px}
  h1{margin:0;font-size:104px;font-weight:500;letter-spacing:-.03em;line-height:1.02}
  p{margin:0;font:400 28px/1.5 "JetBrains Mono",ui-monospace,Menlo,monospace;color:#b9b5ad}
</style>
<div class="brand"><img src="mark.svg" alt="">sandkiln</div>
<h1>Sandboxes for agents</h1>
<p>Run untrusted code in microVMs on your own hardware.</p>
HTML
"$CHROME" --headless=new --no-sandbox --disable-gpu --hide-scrollbars --window-size=1200,630 \
  --virtual-time-budget=4000 --screenshot="$PWD/website/public/og.png" "file://$TMP/og.html" >/dev/null 2>&1
echo "wrote website/public/og.png"
