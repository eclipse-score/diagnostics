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

# Runs the gateway/CDA test against the ECU simulator of the pinned CDA, started
# with the CDA's own testcontainer/docker-compose.yml. Meant for a throwaway
# Linux CI runner: it uses the compose file's fixed 172.42.0.0/16 network and
# publishes the simulator control port 8181 on the host.

set -euo pipefail

if [[ "$(uname -s)" != Linux ]]; then
    echo "This fixture requires a Linux host with access to the Docker bridge" >&2
    exit 1
fi

# The compose file's network gets Docker's default gateway, which the CDA on the
# host uses as its DoIP tester address.
tester_address=172.42.0.1
control=http://127.0.0.1:8181

bazel fetch --repo=@classic-diagnostic-adapter
compose_file="$(bazel info output_base)/external/classic-diagnostic-adapter+/testcontainer/docker-compose.yml"
compose=(docker compose -f "$compose_file" -p "score-diag-ecu-$$")

cleanup() {
    local status=$?
    if ((status != 0)); then
        "${compose[@]}" logs ecu-sim || true
    fi
    if ! "${compose[@]}" down --rmi local; then
        echo "Failed to remove the simulator" >&2
        status=$((status == 0 ? 1 : status))
    fi
    exit "$status"
}
trap cleanup EXIT

"${compose[@]}" up --detach --wait ecu-sim

if ! ip -4 -o addr show | grep -q "inet ${tester_address}/"; then
    echo "Tester address ${tester_address} is not assigned on the host" >&2
    exit 1
fi

# The test seeds a fault, drives the ECU through the gateway and checks what the
# simulator recorded, so every failure lands in the JUnit report the documentation reads.
# The target is manual, and .bazelrc filters manual tests out unless the filter is cleared.
bazel test --config=score_diag_x86_64_linux \
    --test_tag_filters= --cache_test_results=no --test_output=all \
    --test_env=DIAGNOSTICS_TESTER_ADDRESS="$tester_address" \
    --test_env=DIAGNOSTICS_SIMULATOR_CONTROL="$control" \
    --test_filter=simulator_reads_through_gateway \
    -- //score/opensovd-cda:simulator_test
