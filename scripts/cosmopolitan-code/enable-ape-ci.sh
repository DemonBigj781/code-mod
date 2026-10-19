#!/bin/sh
set -eu
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; then
    echo "APE interpreter setup is restricted to disposable GitHub-hosted runners." >&2
    exit 1
fi
sudo install -m 0755 o/cosmopolitan-source/build/bootstrap/ape.elf /usr/bin/ape
if [ ! -e /proc/sys/fs/binfmt_misc/register ]; then
    sudo mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc
fi
if [ ! -e /proc/sys/fs/binfmt_misc/APE ]; then
    printf '%s\n' ':APE:M::MZqFpD::/usr/bin/ape:' | sudo tee /proc/sys/fs/binfmt_misc/register >/dev/null
fi
if [ ! -e /proc/sys/fs/binfmt_misc/APE-jart ]; then
    printf '%s\n' ':APE-jart:M::jartsr::/usr/bin/ape:' | sudo tee /proc/sys/fs/binfmt_misc/register >/dev/null
fi
