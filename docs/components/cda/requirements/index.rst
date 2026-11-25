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


.. _classic_diagnostic_adapter_requirements:

Requirements
############

.. document:: Classic Diagnostic Adapter Requirements
   :id: doc__classic_diagnostic_adapter_requirements
   :status: draft
   :version: 1
   :safety: QM
   :security: YES
   :realizes: wp__requirements_comp
   :tags: requirements, classic_diagnostic_adapter

Functional Requirements
=======================

.. comp_req:: CDA HTTP Server Port
   :id: comp_req__cda__http_server_port
   :reqtype: Interface
   :security: YES
   :safety: QM
   :derived_from: feat_req__diagnostics__classic_adapter
   :status: valid
   :version: 1
   :satisfied_by: comp__classic_diagnostic_adapter

   The classic diagnostic adapter shall serve its SOVD API on the HTTP or HTTPS port set in its configuration.

   Refines :need:`req~sovd-api-http-server-port`.

.. comp_req:: CDA Faults Endpoint
   :id: comp_req__cda__faults
   :reqtype: Interface
   :security: YES
   :safety: QM
   :derived_from: feat_req__diagnostics__classic_adapter
   :status: valid
   :version: 1
   :satisfied_by: comp__classic_diagnostic_adapter

   The classic diagnostic adapter shall, for each ECU:

   * list the ECU's faults (DTCs) with their codes and status at ``GET /components/{ecu}/faults``,
   * return a fault with its status bits, snapshot records and extended data records at
     ``GET /components/{ecu}/faults/{code}``,
   * clear the ECU's fault memory at ``DELETE /components/{ecu}/faults``.

   Refines :need:`req~sovd-api-faults-endpoint`.

.. comp_req:: CDA Asynchronous Operations
   :id: comp_req__cda__operations
   :reqtype: Functional
   :security: YES
   :safety: QM
   :derived_from: feat_req__diagnostics__classic_adapter
   :status: valid
   :version: 1
   :satisfied_by: comp__classic_diagnostic_adapter

   For an ECU routine that defines the ``RequestResults`` and ``Stop`` subfunctions, the classic diagnostic
   adapter shall:

   * start the routine at ``POST /components/{ecu}/operations/{op}/executions`` and answer with an execution
     ID and the status ``running``,
   * request and return the routine's results at ``GET /components/{ecu}/operations/{op}/executions/{id}``,
   * stop the routine at ``DELETE /components/{ecu}/operations/{op}/executions/{id}``, answer with HTTP 200,
     the status ``stopped`` and the Stop result when the ECU returns output data, or with HTTP 204 when it
     returns none, and answer later requests for that execution with HTTP 404.

   Refines :need:`req~sovd-api-operations-handling`.

.. comp_req:: CDA ECU Lock
   :id: comp_req__cda__lock_api
   :reqtype: Interface
   :security: YES
   :safety: QM
   :derived_from: feat_req__diagnostics__classic_adapter
   :status: valid
   :version: 1
   :satisfied_by: comp__classic_diagnostic_adapter

   The classic diagnostic adapter shall let a client acquire a lock on an ECU at
   ``POST /components/{ecu}/locks`` with a lock expiration, answer with the lock's ID, report the lock as
   owned by that client at ``GET /components/{ecu}/locks/{id}``, and release it at
   ``DELETE /components/{ecu}/locks/{id}``.

   Refines :need:`req~sovd-api-lock-api`.

.. comp_req:: CDA ECU Lock Enforcement
   :id: comp_req__cda__lock_enforcement
   :reqtype: Functional
   :security: YES
   :safety: QM
   :derived_from: feat_req__diagnostics__classic_adapter
   :status: valid
   :version: 1
   :satisfied_by: comp__classic_diagnostic_adapter

   The classic diagnostic adapter shall reject a write request to an ECU, such as clearing its faults, with
   HTTP 409 and leave the ECU unchanged when no lock is held on the ECU, and shall execute the request when
   the client holds a lock on the ECU.

   Refines :need:`req~sovd-api-lock-ecu-enforcement`.

.. comp_req:: CDA UDS Service Mapping
   :id: comp_req__cda__resource_mapping
   :reqtype: Interface
   :security: YES
   :safety: QM
   :derived_from: feat_req__diagnostics__classic_adapter
   :status: valid
   :version: 1
   :satisfied_by: comp__classic_diagnostic_adapter

   The classic diagnostic adapter shall translate the following SOVD resources of an ECU into the UDS
   services of that ECU:

   * ``/data/{data-identifier}`` reads to ReadDataByIdentifier (SID 22\ :sub:`16`),
   * ``/faults`` reads to ReadDTCInformation (SID 19\ :sub:`16`) and clearing to ClearDiagnosticInformation
     (SID 14\ :sub:`16`),
   * ``/modes/session`` updates to DiagnosticSessionControl (SID 10\ :sub:`16`),
   * ``/operations/reset`` executions to ECUReset (SID 11\ :sub:`16`),
   * other ``/operations/{op}`` executions to RoutineControl (SID 31\ :sub:`16`).

   Refines :need:`req~sovd-api-standardized-resource-collection-mapping`.
