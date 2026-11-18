#!/usr/bin/env bash
set -euo pipefail

dry=0
if [ "${1:-}" = "--dry-run" ]; then
    dry=1
    shift
fi

commit=$(git rev-parse "${1:-HEAD}^{commit}")
dir=docs/changes
repo=${GITHUB_REPOSITORY:-STEALTHNET-APP/STEALTHNET-RW-SYSTEM}

base=$(git show "$commit:Cargo.toml" | sed -n 's/^version = "\([0-9][0-9]*\.[0-9][0-9]*\)\.[0-9][0-9]*"$/\1/p' | sed -n '1p')
if [ -z "$base" ]; then
    echo "в Cargo.toml нет версии" >&2
    exit 1
fi
version="$base.$(git rev-list --count "$commit")"
tag="v$version"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    echo "релиз $tag уже есть"
    exit 0
fi

newest=$(git tag --list 'v*' --sort=-v:refname | sed -n '1p')
if [ -n "$newest" ] && [ "$(printf '%s\n%s\n' "$newest" "$tag" | sort -V | tail -n 1)" != "$tag" ]; then
    echo "уже вышел более новый релиз $newest, $tag не выпускается: его изменения вошли туда"
    exit 0
fi

prev=$(git tag --list 'v*' --merged "$commit" --sort=-v:refname | sed -n '1p')
if [ -n "$prev" ]; then
    files=$(git diff --name-only --diff-filter=A "$prev" "$commit" -- "$dir")
else
    files=$(git ls-tree -r --name-only "$commit" -- "$dir")
fi
files=$(printf '%s\n' "$files" | grep -E "^$dir/[^/]+\.md$" | grep -v "^$dir/README\.md$" | sort || true)

if [ -z "$files" ]; then
    echo "с ${prev:-начала истории} нет новых заметок в $dir, релиз $tag не выпускается"
    exit 0
fi

body=$(mktemp)
trap 'rm -f "$body"' EXIT
titles=()
while IFS= read -r file; do
    text=$(git show "$commit:$file")
    first=$(printf '%s\n' "$text" | sed -n '1p')
    case "$first" in
        "# "*)
            heading=${first#"# "}
            rest=$(printf '%s\n' "$text" | sed '1d' | sed '/./,$!d')
            ;;
        *)
            heading=$(basename "$file" .md)
            rest=$text
            ;;
    esac
    titles+=("$heading")
    printf '## %s\n\n%s\n\n' "$heading" "$rest" >>"$body"
done <<<"$files"

if [ -n "$prev" ]; then
    printf '[Изменения в коде с %s](https://github.com/%s/compare/%s...%s)\n' "$prev" "$repo" "$prev" "$tag" >>"$body"
fi

title="$version — ${titles[0]}"
for extra in "${titles[@]:1:2}"; do
    title="$title · $extra"
done
if [ "${#titles[@]}" -gt 3 ]; then
    title="$title и ещё $((${#titles[@]} - 3))"
fi

if [ "$dry" = 1 ]; then
    printf '%s\n%s\n\n' "$tag" "$title"
    cat "$body"
    exit 0
fi

for attempt in 1 2 3; do
    if gh release create "$tag" --target "$commit" --title "$title" --notes-file "$body" --latest; then
        exit 0
    fi
    sleep $((attempt * 20))
    if gh release view "$tag" >/dev/null 2>&1; then
        echo "релиз $tag уже создан"
        exit 0
    fi
    echo "релиз $tag не создан, попытка $attempt из 3" >&2
done
exit 1
