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

Classic Diagnostic Adapter
##########################

.. document:: Classic Diagnostic Adapter
   :id: doc__classic_diagnostic_adapter
   :status: draft
   :version: 1
   :safety: QM
   :security: YES
   :tags: classic_diagnostic_adapter
   :realizes: wp__cmpt_request

.. toctree::
   :hidden:

   requirements/index.rst

Abstract
========

The classic diagnostic adapter (CDA, ``//score/opensovd-cda:opensovd-cda``) of Eclipse OpenSOVD translates SOVD
requests into UDS for classic ECUs. It is one of the upstream SOVD servers that the
:doc:`SOVD gateway <../sovd_gateway/index>` can forward to.

Motivation and Rationale
========================

The CDA is specified and developed in https://github.com/eclipse-opensovd/classic-diagnostic-adapter. Its
requirements are built from the pinned CDA sources (``//score/opensovd-cda:needs_json``) and referenced
directly. The requirements below state the CDA behavior that the v1.0 integration relies on, refine the
upstream CDA requirement they come from, and trace them to :need:`feat_req__diagnostics__classic_adapter`.

Component
=========

.. comp:: Classic Diagnostic Adapter
   :id: comp__classic_diagnostic_adapter
   :security: YES
   :safety: QM
   :status: valid
   :version: 1
   :tags: classic_diagnostic_adapter
   :belongs_to: feat__diagnostics

Specification
=============

Each requirement states the behavior the integration relies on; the refined CDA requirement gives the full
upstream specification.

* :need:`comp_req__cda__http_server_port`
* :need:`comp_req__cda__faults`
* :need:`comp_req__cda__operations`
* :need:`comp_req__cda__lock_api`
* :need:`comp_req__cda__lock_enforcement`
* :need:`comp_req__cda__resource_mapping`
