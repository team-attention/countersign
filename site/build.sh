#!/bin/sh
# Assemble public/ from the repo's pages: landing + SEO files from site/static, ledger from web/, replay from docs/demo/.
set -e
cd "$(dirname "$0")"
rm -rf public && mkdir -p public
cp static/* public/
cp ../web/index.html public/ledger.html
cp ../docs/demo/replay.html public/replay.html
cp ../docs/demo/demo-run.jsonl public/demo-run.jsonl
