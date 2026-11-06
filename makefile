SHELL := /bin/sh

PYTHON ?= python3
WORKSPACE := code-rs
CARGO_TARGET_DIR ?= $(abspath $(WORKSPACE)/target)
COSMO_COMMAND := $(PYTHON) scripts/cosmopolitan-code/commands.py
COSMO_OUTPUT ?=
COSMO_OUTPUT_ARG := $(if $(strip $(COSMO_OUTPUT)),--output "$(COSMO_OUTPUT)",)

export CARGO_TARGET_DIR

.PHONY: all build build-auto build-github build-local check test test-build run \
	test-browser test-keyring test-tui cache-report verify-fast-cache cull-debug

all: build

# Canonical deliverable build. Compilation and tests for this repository run
# only through GitHub Actions against the exact intended source revision.
build: build-auto

build-auto:
	$(COSMO_COMMAND) build-auto $(COSMO_OUTPUT_ARG)

build-github:
	$(COSMO_COMMAND) build-action $(COSMO_OUTPUT_ARG)

build-local:
	$(COSMO_COMMAND) build-local $(COSMO_OUTPUT_ARG)

check:
	@echo "Local tests are disabled for this repository; dispatch GitHub Actions." >&2
	@exit 1

test:
	@echo "Local tests are disabled for this repository; dispatch GitHub Actions." >&2
	@exit 1

test-build: build test

run:
	$(COSMO_COMMAND) run $(COSMO_OUTPUT_ARG) -- $(ARGS)

test-browser:
	@echo "Local tests are disabled for this repository; dispatch GitHub Actions." >&2
	@exit 1

test-keyring:
	@echo "Local tests are disabled for this repository; dispatch GitHub Actions." >&2
	@exit 1

test-tui:
	@echo "Local tests are disabled for this repository; dispatch GitHub Actions." >&2
	@exit 1

cache-report:
	@du -sh $(WORKSPACE)/target/dev-fast $(WORKSPACE)/target/debug .code/working/_target-cache 2>/dev/null || true
	@if command -v sccache >/dev/null 2>&1; then sccache --show-stats; fi

verify-fast-cache:
	@test -d $(WORKSPACE)/target/dev-fast
	@test -d $(WORKSPACE)/target/dev-fast/deps
	@echo "Preserving existing dev-fast cache at $(WORKSPACE)/target/dev-fast"

# Preserve a verified dev-fast build before deleting incompatible debug-profile
# artifacts. Cargo profile outputs cannot be safely merged by copying files.
# The explicit confirmation prevents accidental removal from a plain `make`.
cull-debug: verify-fast-cache
	@test "$(CONFIRM)" = "delete-code-rs-target-debug" || { \
		echo "Refusing to delete $(WORKSPACE)/target/debug." >&2; \
		echo "Re-run: make cull-debug CONFIRM=delete-code-rs-target-debug" >&2; \
		exit 1; \
	}
	rm -rf $(WORKSPACE)/target/debug
