#!/bin/sh
set -efu
umask 077
export LC_ALL=C
media=/mnt/us
die() { printf 'Private fixture mount: %s\n' "$1" >&2; exit 1; }
plain() { [ -f "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ] && [ "$(stat -c '%u:%h' "$1")" = 0:1 ]; }
directory() { [ -d "$1" ] && [ ! -L "$1" ] && [ "$(readlink -f "$1")" = "$1" ] && [ "$(stat -c %u "$1")" = 0 ]; }
present() { [ -e "$1" ] || [ -L "$1" ]; }
positive() { case "$1" in ''|0*|*[!0-9]*) return 1;; esac; [ "${#1}" -le 20 ]; }
[ "$#" -ge 4 ] && [ "$(id -u)" = 0 ] || die 'Expected a root-owned run, boot, phase and private mount owner.'
run=$1; boot=$2; phase=$3; owner=$4; shift 4
case "$run" in bili-shelf-*) suffix=${run#bili-shelf-};; *) die 'Invalid run.';; esac
case "$suffix" in *[!0-9a-f]*|'') die 'Invalid run.';; esac
case "$owner" in *[!0-9a-f]*|'') die 'Invalid mount owner.';; esac
[ "${#owner}" -eq 32 ] || die 'Invalid mount owner.'
[ "${#suffix}" -eq 32 ] && [ "${#boot}" -eq 36 ] && [ "$(cat /proc/sys/kernel/random/boot_id)" = "$boot" ] || die 'Boot or run changed.'
stage=$media/$run
data=$stage/data
mountpoint=$data/account/private
object=$mountpoint/session.json
seal=$mountpoint/.fixture-owner
tag=$(printf '%.16s' "$owner"); private_owner=${owner#"$tag"}
label=bili-private-$suffix-$tag
for part in "$media" "$stage" "$data" "$data/account" "$mountpoint"; do directory "$part" || die 'Unowned stage path.'; done
plain "$stage/run-id" && printf '%s\n' "$run" | cmp -s - "$stage/run-id" || die 'Unbound disposable run.'

mount_identity() {
    awk -v path="$mountpoint" -v label="$label" -v readonly="${1:-0}" '
        index($5,path "/")==1 { bad=1 }
        { separator=7; while(separator<=NF && $separator!="-") ++separator;
          if ($(separator+2)==label && $5!=path) bad=1;
          if ($5==path) {
            count++; if (separator+3>NF || $(separator+1)!="tmpfs" || $(separator+2)!=label || $4!="/" || $1!~/^[1-9][0-9]*$/ || $3!~/^[0-9]+:[0-9]+$/) bad=1;
            flags="," $6 ","; if ((flags!~/,rw,/ && flags!~/,ro,/) || (readonly && flags!~/,ro,/) || flags!~/,nosuid,/ || flags!~/,noexec,/ || flags!~/,nodev,/) bad=1;
            superflags="," $(separator+3) ","; if (superflags!~/,size=64k,/) bad=1;
            token=$1 ":" $3;
          }
        }
        END { if (bad || count>1) exit 1; if(count==1) print token; else print "absent" }
    ' /proc/self/mountinfo
}

contents() (
    set +f
    for leaf in "$mountpoint"/* "$mountpoint"/.[!.]* "$mountpoint"/..?*; do
        present "$leaf" || continue
        if [ "$leaf" = "$object" ]; then plain "$leaf" || exit 1
        elif [ "${1:-base}" = mounted ] && [ "$leaf" = "$seal" ]; then plain "$leaf" || exit 1
        else exit 1; fi
    done
)

mounted_witness() {
    current=$(mount_identity "${1:-0}") && [ "$current" != absent ] && [ "$(stat -c '%a:%u' "$mountpoint")" = 700:0 ] && contents mounted || return 1
    plain "$seal" && [ "$(stat -c '%a:%s' "$seal")" = 600:50 ] && IFS=' ' read -r binding generation excess <"$seal" || return 1
    [ "$binding" = "$private_owner" ] && [ -z "$excess" ] && [ "${#generation}" -eq 32 ] || return 1
    case "$generation" in ''|*[!0-9a-f]*) return 1;; esac
    printf '%s %s\n' "$private_owner" "$generation" | cmp -s - "$seal" || return 1
    printf '%s:%s' "$current" "$generation"
}

base_proof() {
    [ "$(mount_identity)" = absent ] && contents && plain "$object" || return 1
    bytes=$(stat -c %s "$object"); positive "$bytes" && [ "$bytes" -le 4096 ] || return 1
    parent_id=$(stat -c '%d:%i:%f:%s:%Y' "$data/account") && dir_id=$(stat -c '%d:%i:%f:%s:%Y' "$mountpoint") && file_id=$(stat -c '%d:%i:%f:%s:%Y' "$object") || return 1
    printf '%s/%s/%s' "$parent_id" "$dir_id" "$file_id"
}

attach_failure() {
    if verify_attach_identity && ! present "$stage/picked" && ! present "$stage/armed" && ! present /tmp/bilicomics-launcher.lock; then
        if umount "$mountpoint" && [ "$(base_proof)" = "$proof" ]; then die 'Setup failed; its sealed mount was removed and the original restored.'; fi
    fi
    die 'Setup stopped without proving removal; use the original run for recovery.'
}

verify_attach_identity() {
    [ -n "${identity:-}" ] && [ "$(mounted_witness)" = "$identity:$generation" ]
}

case "$phase:$#" in
    inspect-base:0)
        proof=$(base_proof) || die 'Underlying mock storage is not an admitted regular tree.'
        printf 'base=%s\n' "$proof";;
    attach:1)
        proof=$(base_proof) && [ "$proof" = "$1" ] || die 'Underlying object changed before mount.'
        if present "$stage/picked" || present "$stage/armed"; then die 'Never mount after route activation or click.'; fi
        exec 8<"$object" 9<"$object"
        [ "$(stat -Lc '%d:%i' /proc/self/fd/8)" = "$(stat -c '%d:%i' "$object")" ] && [ "$(base_proof)" = "$proof" ] || die 'Underlying mock handle changed.'
        generation=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
        [ "${#generation}" -eq 32 ] || die 'No private mount generation available.'
        case "$generation" in *[!0-9a-f]*) die 'Invalid private mount generation.';; esac
        trap '' HUP
        mount -t tmpfs -o rw,nosuid,nodev,noexec,size=64k,mode=0700 "$label" "$mountpoint" || die 'Could not create run-private tmpfs.'
        identity=$(mount_identity) && [ "$identity" != absent ] && [ "$(stat -c '%a:%u' "$mountpoint")" = 700:0 ] && contents && ! present "$object" || die 'Mounted object requires scoped recovery.'
        (set -C; printf '%s %s\n' "$private_owner" "$generation" >"$seal") && verify_attach_identity || attach_failure
        (set -C; cat <&8 >"$object") && plain "$object" && [ "$(stat -c %a "$object")" = 600 ] && cmp -s - "$object" <&9 || attach_failure
        mount -o remount,ro,nosuid,nodev,noexec "$mountpoint" || attach_failure
        [ "$(mounted_witness 1)" = "$identity:$generation" ] || attach_failure
        printf 'mounted=%s:%s\n' "$identity" "$generation";;
    inspect-ready:0)
        identity=$(mounted_witness 1) && plain "$object" && [ "$(stat -c %a "$object")" = 600 ] || die 'No sealed root-only mock session.'
        bytes=$(stat -c %s "$object"); positive "$bytes" && [ "$bytes" -le 4096 ] || die 'Invalid mounted session object.'
        printf 'mounted=%s\n' "$identity";;
    inspect-mounted:0)
        identity=$(mounted_witness) || die 'Unrecognized, unbound or absent mount.'
        if present "$object"; then plain "$object" && [ "$(stat -c %a "$object")" = 600 ] || die 'Unrecognized mounted session.'; fi
        printf 'mounted=%s\n' "$identity";;
    detach:2)
        identity=$(mounted_witness) && [ "$identity" = "$1" ] || die 'Will not unmount an absent, foreign or replaced filesystem.'
        [ ! -e "$stage/armed" ] && [ ! -L "$stage/armed" ] && ! present /tmp/bilicomics-launcher.lock || die 'Will not unmount an armed or active route.'
        directory "$mountpoint" && [ "$(stat -c '%a:%u' "$mountpoint")" = 700:0 ] || die 'Mounted root changed before removal.'
        trap '' HUP
        umount "$mountpoint" || die 'Unmount refused; do not lazy or force unmount.'
        proof=$(base_proof) && [ "$proof" = "$2" ] || die 'Original underlying object not verified after unmount.'
        printf 'restored\n';;
    *) die 'Invalid phase or witness.';;
esac
