..
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


.. _diagnostics_module:

Diagnostics Module
==================

.. mod:: Diagnostics Module
   :id: mod__diagnostics
   :version: 1
   :security: YES
   :safety: QM
   :status: valid
   :maturity: preview
   :includes: comp__classic_diagnostic_adapter[version==1], comp__fault_lib[version==1], comp__fault_manager[version==1], comp__sovd_gateway[version==1], comp__uds_data_bridge[version==1]

The diagnostics module provides the S-CORE diagnostic services: the SOVD gateway for testers, the classic
diagnostic adapter for UDS ECUs, the UDS data bridge, and the fault library and fault manager that collect and
store diagnostic faults.
