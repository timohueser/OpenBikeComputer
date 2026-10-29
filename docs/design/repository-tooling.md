---
system: repository-tooling
kind: system
area: Tooling
purpose: "Keeps the development workflow running: the `obc` command, test selection, CI gates, fixtures."
owns:
  - tools/obc
  - tools/build-ios-host.sh
  - tools/build-planner-ios.sh
  - tools/install-geos.sh
  - tools/install-riscv.sh
  - tools/obc-dev.sh
  - tools/ready.py
  - tools/test_plan.py
  - tools/ci_*.py
  - tools/governs.py
  - tools/prose.py
  - tools/cargo_scope.py
  - tools/dev_cleanup.py
  - tools/tasks.py
  - tools/board.py
  - tools/fixtures.py
  - tools/fixture_catalog.py
  - tools/check_fixture_policy.py
  - tools/cairn/**
  - tools/ci/**
  - tools/licenses/**
  - tools/vps/**
  - tools/tests/**
  - tools/testing/**
  - firmware/tools/check_dependencies.py
  - firmware/tools/resource_guard.py
  - firmware/tools/strict_align_probe.rs
  - firmware/tools/tests/__init__.py
  - firmware/tools/tests/test_resource_guard.py
  - firmware/tools/tests/test_dependencies.py
  - firmware/tools/tests/test_fs7_board_composition.py
  - host/obc-fixtures/**
  - host/obc-inspect/**
  - fixtures/**
  - .github/**
  - scripts/**
depends_on:
  - card-storage
  - device-runtime
  - foundations
  - map-reading
  - navigation
test_depends_on: []
status: draft
---

# Repository tooling

The body of this document is not written yet.
