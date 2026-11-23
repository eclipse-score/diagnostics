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

"""Browse the real gateway from a separate network namespace (ISO 17978-3 §5.11)."""

import argparse
import http.client
import json
import os
import queue
import signal
import time
from typing import TypedDict, cast
from urllib.parse import urlsplit

from zeroconf import IPVersion, ServiceBrowser, ServiceStateChange, Zeroconf

SERVICE_TYPE = "_sovd._tcp.local."
GATEWAY_ADDRESS = "192.0.2.1"
CLIENT_ADDRESS = "192.0.2.2"


class SovdInfo(TypedDict):
    version: str


class VersionInfo(TypedDict):
    sovd_info: list[SovdInfo]


class Arguments(argparse.Namespace):
    host: str = ""
    gateway_pid: int = 0


def browse(host: str, gateway_pid: int) -> None:
    events: queue.Queue[tuple[str, ServiceStateChange]] = queue.Queue()

    def changed(
        zeroconf: Zeroconf,
        service_type: str,
        name: str,
        state_change: ServiceStateChange,
    ) -> None:
        del zeroconf, service_type
        events.put((name, state_change))

    fullname = f"{host}.{SERVICE_TYPE}"

    def wait_for(expected: ServiceStateChange, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        while (remaining := deadline - time.monotonic()) > 0:
            try:
                name, state = events.get(timeout=remaining)
            except queue.Empty:
                break
            if name == fullname and state == expected:
                return
        raise AssertionError(f"Did not observe {expected.name} for {fullname}")

    with Zeroconf(interfaces=[CLIENT_ADDRESS], ip_version=IPVersion.V4Only) as client:
        browser = ServiceBrowser(client, SERVICE_TYPE, handlers=[changed])
        try:
            wait_for(ServiceStateChange.Added, 25)
            info = client.get_service_info(SERVICE_TYPE, fullname, timeout=5000)
            assert info is not None, f"Could not resolve {fullname}"
            assert info.server == f"{host}.local.", info
            assert info.port == 7690, info
            assert info.parsed_addresses() == [GATEWAY_ADDRESS], info
            assert info.properties[b"identification"] == host.encode(), info
            accessurl = info.properties[b"accessurl"]
            assert accessurl is not None, info
            assert accessurl.decode() == f"http://{host}.local:7690/sovd", info

            # Use DNS-SD's resolved address, not the runner's NSS/mDNS setup.
            # The discovery root is the parent of version-info (§5.11.4).
            url = urlsplit(accessurl.decode())
            connection = http.client.HTTPConnection(
                info.parsed_addresses()[0], info.port, timeout=5
            )
            try:
                connection.request(
                    "GET", f"{url.path}/version-info", headers={"Host": url.netloc}
                )
                response = connection.getresponse()
                body = response.read()
                assert response.status == 401, (response.status, body)
                connection.request(
                    "GET",
                    f"{url.path}/version-info",
                    headers={
                        "Host": url.netloc,
                        "Authorization": f"Bearer {os.environ['SOVD_AUTH_TOKEN']}",
                    },
                )
                response = connection.getresponse()
                body = response.read()
                assert response.status == 200, (response.status, body)
                version_info = cast(VersionInfo, json.loads(body))
                assert version_info["sovd_info"], body
                assert version_info["sovd_info"][0]["version"], body
            finally:
                connection.close()

            print(f"Discovered and reached {accessurl.decode()}", flush=True)
            os.kill(gateway_pid, signal.SIGTERM)
            # A fresh cache entry must disappear on goodbye, not after its TTL.
            wait_for(ServiceStateChange.Removed, 5)
            print("Gateway withdrawn after SIGTERM", flush=True)
        finally:
            browser.cancel()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--host", required=True)
    _ = parser.add_argument("--gateway-pid", type=int, required=True)
    args = Arguments()
    _ = parser.parse_args(namespace=args)
    if args.gateway_pid <= 1:
        parser.error("--gateway-pid must name the gateway child process")
    browse(args.host, args.gateway_pid)
