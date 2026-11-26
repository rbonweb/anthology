#!/bin/sh
# Renders Formula/anthology-installer.rb on stdout from packaging/homebrew/anthology-installer.rb.in.
#
#   ci/jobs/installer-formula.sh VERSION CHECKSUMS_FILE BASE_URL
#
# VERSION is the installer's crate version (no tag prefix). CHECKSUMS_FILE is the release's
# checksums.txt (`<sha256>  anthology-installer-<target>.tar.gz`, one line per target). BASE_URL is
# where the tarballs are: an https URL, or a file:// URL for the pull-request dry run that installs
# the formula from the tarballs it just built. Nothing is printed unless every target has a
# well-formed checksum, so a half-filled formula is never published.
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: installer-formula.sh VERSION CHECKSUMS_FILE BASE_URL" >&2
    exit 2
fi
version=$1
checksums=$2
base=$3

root=$(cd "$(dirname "$0")/../.." && pwd)
template=$root/packaging/homebrew/anthology-installer.rb.in

case $version in
    '' | *[!0-9A-Za-z.+-]* | .* | -*)
        echo "installer-formula: bad version '$version'" >&2
        exit 1
        ;;
esac
case $version in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *)
        echo "installer-formula: version '$version' is not <major>.<minor>.<patch>" >&2
        exit 1
        ;;
esac
case $base in
    https://* | file:///*) ;;
    *)
        echo "installer-formula: the base URL must be https:// or file:///, not '$base'" >&2
        exit 1
        ;;
esac
case $base in
    *[!0-9A-Za-z./:_-]*)
        echo "installer-formula: the base URL has an unexpected character" >&2
        exit 1
        ;;
esac
base=${base%/}
if [ ! -f "$checksums" ] || [ ! -f "$template" ]; then
    echo "installer-formula: missing $checksums or $template" >&2
    exit 1
fi

sums=
for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    sha=$(awk -v f="anthology-installer-$target.tar.gz" '$2 == f { print $1 }' "$checksums")
    case $sha in
        *[!0-9a-f]* | '')
            echo "installer-formula: no sha256 for $target in $checksums" >&2
            exit 1
            ;;
    esac
    if [ "${#sha}" -ne 64 ]; then
        echo "installer-formula: the sha256 for $target is not 64 hex digits" >&2
        exit 1
    fi
    name=$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')
    sums="$sums -e s|@SHA_${name}@|$sha|g"
done

# The substituted values hold none of sed's special characters: the version, the URL and the
# checksums were all checked above ('|' and '&' and a backslash are not allowed in any of them).
# shellcheck disable=SC2086 # $sums is a list of sed options without spaces inside them
sed -e "s|@VERSION@|$version|g" -e "s|@BASE_URL@|$base|g" $sums "$template"
