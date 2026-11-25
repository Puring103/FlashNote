#!/bin/sh
set -eu
umask 077

usage() {
    echo 'Usage: sh data-backup.sh {backup|restore} /absolute/path/to/backup.tar.gz' >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
archive=$2
case "$archive" in /*) ;; *) usage ;; esac
cd "$(dirname "$0")"

run_helper() {
    docker compose run --rm --no-deps -T --entrypoint "$@"
}

validate_archive() {
    tar -tzf "$1" .env casdoor/data/casdoor.db sync/sync.sqlite3 >/dev/null
}

case "$1" in
backup)
    [ -f .env ] && [ -d .casdoor ] || { echo 'Run from an initialized deployment.' >&2; exit 1; }
    [ ! -e "$archive" ] || { echo 'Backup path already exists.' >&2; exit 1; }
    running=$(docker compose ps --status running --services sync casdoor caddy)
    partial=$(mktemp "${archive}.XXXXXX")
    cleanup() {
        status=$?
        trap - EXIT HUP INT TERM
        rm -f -- "$partial"
        set --
        for service in casdoor caddy sync; do
            if printf '%s\n' "$running" | grep -qx "$service"; then
                set -- "$@" "$service"
            fi
        done
        if [ "$#" -gt 0 ]; then docker compose start "$@" || status=1; fi
        exit "$status"
    }
    trap cleanup EXIT
    trap 'exit 1' HUP INT TERM
    docker compose stop sync casdoor caddy
    run_helper tar backup -C /backup -czf - .env casdoor sync caddy-data caddy-config > "$partial"
    validate_archive "$partial"
    ln -T -- "$partial" "$archive"
    ;;
restore)
    [ ! -e .env ] && [ ! -e .casdoor ] || { echo 'Restore into a fresh deployment checkout.' >&2; exit 1; }
    : "${COMPOSE_PROJECT_NAME:?Set COMPOSE_PROJECT_NAME to a new project name for recovery}"
    filter="label=com.docker.compose.project=$COMPOSE_PROJECT_NAME"
    volumes=$(docker volume ls --quiet --filter "$filter")
    containers=$(docker ps --all --quiet --filter "$filter")
    [ -z "$volumes$containers" ] || { echo 'The requested Compose project already has resources.' >&2; exit 1; }
    validate_archive "$archive"
    partial=$(mktemp .env.restore.XXXXXX)
    trap 'rm -f -- "$partial"' EXIT
    trap 'exit 1' HUP INT TERM
    tar -xOzf "$archive" .env > "$partial"
    [ -s "$partial" ]
    ln -T -- "$partial" .env
    run_helper tar backup -C /backup --numeric-owner --same-owner --exclude=.env -xzf - < "$archive"
    echo 'Restored data. Start this Compose project with the same COMPOSE_PROJECT_NAME.'
    ;;
*) usage ;;
esac
