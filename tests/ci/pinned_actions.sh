#!/bin/sh
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
loose=$(grep -nE '^[[:space:]]*(-[[:space:]]*)?uses:' "$ROOT"/.github/workflows/*.yml \
    | grep -vE 'uses:[[:space:]]*[^[:space:]@]+@[0-9a-f]{40}[[:space:]]*$')
if [ -n "$loose" ]; then
    printf 'действия CI без закреплённого хеша коммита:\n%s\n' "$loose"
    exit 1
fi
echo "все действия CI закреплены хешем коммита"
