#!/usr/bin/env bash
# *******************************************************************************
# Copyright (c) 2026 Contributors to the Eclipse Foundation
#
# See the NOTICE file(s) distributed with this work for additional
# information regarding copyright ownership.
#
# This program and the accompanying materials are made available under the
# terms of the Apache License Version 2.0 which is available at
# https://www.apache.org/licenses/LICENSE-2.0
#
# SPDX-License-Identifier: Apache-2.0
# *******************************************************************************

set -euo pipefail

gateway="$(realpath "$1")"
browser="$(realpath "$2")"

# Keep the multicast link private to the test. Only this fixture needs privileges;
# Bazel itself and the build still run as the normal user.
if (( EUID != 0 )); then
    exec sudo -n -- env \
        RUNFILES_DIR="${RUNFILES_DIR:-${TEST_SRCDIR:?}}" \
        TEST_TMPDIR="${TEST_TMPDIR:?}" \
        bash "$(realpath "$0")" "$gateway" "$browser"
fi

suffix="$$-${RANDOM}"
server="mdns-server-${suffix}"
client="mdns-client-${suffix}"
host="score-test-${suffix}"
test_token="mdns-test-token"
log="${TEST_TMPDIR:?}/gateway.log"
gateway_pid=""

cleanup() {
    local status=$?
    trap - EXIT
    for namespace in "$client" "$server"; do
        # Also stop a browser interrupted by the outer test timeout. Never kill
        # a stale PID that could already belong to an unrelated process.
        ip netns pids "$namespace" 2>/dev/null | xargs -r kill -KILL 2>/dev/null || true
        ip netns delete "$namespace" 2>/dev/null || true
    done
    if [[ -n "$gateway_pid" ]]; then
        wait "$gateway_pid" 2>/dev/null || true
    fi
    if (( status != 0 )) && [[ -f "$log" ]]; then
        cat "$log" >&2
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ip netns add "$server"
ip netns add "$client"
ip link add eth0 netns "$server" type veth peer name eth0 netns "$client"
ip -n "$server" address add 192.0.2.1/24 dev eth0
ip -n "$client" address add 192.0.2.2/24 dev eth0
for namespace in "$server" "$client"; do
    ip -n "$namespace" link set lo up
    ip -n "$namespace" link set eth0 up
done

ip netns exec "$server" env \
    SOVD_URL=http://0.0.0.0:7690/sovd \
    SOVD_MDNS="$host" \
    SOVD_MDNS_HOST="$host" \
    SOVD_MDNS_INTERFACE=eth0 \
    SOVD_UPSTREAM_URL= \
    SOVD_AUTH_TOKEN="$test_token" \
    "$gateway" >"$log" 2>&1 &
gateway_pid=$!

timeout 45s ip netns exec "$client" env SOVD_AUTH_TOKEN="$test_token" "$browser" \
    --host "$host" --gateway-pid "$gateway_pid"
wait "$gateway_pid"
gateway_pid=""
