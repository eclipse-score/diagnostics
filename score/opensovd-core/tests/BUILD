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

load("@rules_python//python:py_binary.bzl", "py_binary")
load("@rules_shell//shell:sh_test.bzl", "sh_test")

exports_files(
    ["auth.rs"],
    visibility = ["//score/opensovd-core:__pkg__"],
)

py_binary(
    name = "mdns_browse_client",
    testonly = True,
    srcs = ["mdns_browse.py"],
    main = "mdns_browse.py",
    deps = ["@gateway_test_deps//zeroconf"],
)

sh_test(
    name = "mdns_browse_test",
    size = "small",
    srcs = ["mdns_browse.sh"],
    args = [
        "$(rootpath //score/opensovd-core:opensovd-gateway)",
        "$(rootpath :mdns_browse_client)",
    ],
    data = [
        ":mdns_browse_client",
        "//score/opensovd-core:opensovd-gateway",
    ],
    local = True,
    tags = [
        "manual",
        "requires-network",
    ],
    target_compatible_with = ["@platforms//os:linux"],
)
