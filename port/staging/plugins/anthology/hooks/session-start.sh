#!/bin/sh
# session-start.sh: the anthology SessionStart hook (design §8.7, plan GC-7). It installs the
# pinned binary when needed (one download at most, 20 s), then hands over to
# `anthology hook session-start`. It always exits 0 and prints exactly one JSON line.
ANTHOLOGY_HOOK_T0=$(date +%s)
export ANTHOLOGY_HOOK_T0
case $0 in
    */*) anth_hooks=${0%/*} ;;
    *) anth_hooks=. ;;
esac
anth_incomplete() {
    printf '{"systemMessage":"anthology: session start did not complete (%s); the routing guide may be missing"}\n' "$1"
}
if [ ! -f "$anth_hooks/lib.sh" ] || ! (command . "$anth_hooks/lib.sh") >/dev/null 2>&1; then
    anth_incomplete 'hooks/lib.sh did not load'
    exit 0
fi
# shellcheck source=lib.sh
command . "$anth_hooks/lib.sh"
anth_paths "$anth_hooks/.."
anth_st=$(anth_bootstrap_main session --plugin anthology --quiet --budget 20 </dev/null 2>/dev/null)
case $anth_st in
    status=present) ANTHOLOGY_BOOTSTRAP_DOWNLOADED=0 ;;
    status=installed) ANTHOLOGY_BOOTSTRAP_DOWNLOADED=1 ;;
    status=failed:?*)
        anth_system_message anthology "${anth_st#status=failed:}"
        exit 0
        ;;
    *)
        anth_incomplete 'the bootstrap gave no status'
        exit 0
        ;;
esac
export ANTHOLOGY_BOOTSTRAP_DOWNLOADED
if ! anth_resolve "$anth_hooks/.." || ! anth_installed; then
    anth_incomplete 'the anthology binary is missing'
    exit 0
fi
anth_out=$("$ANTH_BIN" hook session-start; printf 'x%s' "$?")
anth_rc=${anth_out##*x}
anth_out=${anth_out%x*}
while :; do
    case $anth_out in
        *"$ANTH_NL") anth_out=${anth_out%"$ANTH_NL"} ;;
        *) break ;;
    esac
done
if [ "$anth_rc" = 0 ] && [ -n "$anth_out" ]; then
    printf '%s\n' "$anth_out"
else
    anth_incomplete "exit $anth_rc"
fi
exit 0
