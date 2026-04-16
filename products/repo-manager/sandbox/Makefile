# repo-manager sandbox — container wrapper.
#
# Primary runtime: podman (rootless, daemonless, Apache-2.0).
# Fallback:        docker engine (Linux only).
#
# Usage:
#   make sandbox-build        # build the OCI image
#   make sandbox-up           # start the container (detached)
#   make sandbox-shell        # attach to tmux session with claude-cli
#   make sandbox-down         # stop the container
#   make sandbox-reset        # stop, remove, and rebuild from scratch
#   make sandbox-logs         # follow entrypoint stdout/stderr
#   make sandbox-status       # show container state
#
# Override runtime on systems without podman:
#   CONTAINER_CLI=docker make sandbox-up
#

# ------------------------------------------------------------------
# Config
# ------------------------------------------------------------------
CONTAINER_CLI ?= podman
IMAGE_NAME    ?= konf-repo-manager
IMAGE_TAG     ?= latest
CONTAINER     ?= konf-repo-manager

# Repo root (konf-dev-stack) = four levels up from sandbox/.
#   sandbox/ → products/repo-manager/ → products/ → konf/ → konf-dev-stack/
REPO_ROOT     ?= $(abspath $(CURDIR)/../../../..)
MOUNT_TARGET  ?= /home/konf-agent/konf-dev-stack

# Pass through the host's UID/GID so bind-mounted files don't become
# root-owned on edit.
HOST_UID      ?= $(shell id -u)
HOST_GID      ?= $(shell id -g)

# Env file (gitignored). Copy .env.example, fill in your secrets.
ENV_FILE      ?= $(CURDIR)/.env

# Named volumes for caches that should persist across rebuilds.
VOL_CARGO     ?= konf-repo-manager-cargo
VOL_NPM       ?= konf-repo-manager-npm

# Additional args for run (resource limits, nets, etc.)
RUN_EXTRA     ?=

# ------------------------------------------------------------------
# Helpers
# ------------------------------------------------------------------
.PHONY: _require_env
_require_env:
	@if [ ! -f "$(ENV_FILE)" ]; then \
	  echo "ERROR: $(ENV_FILE) missing. cp .env.example .env and fill it in."; \
	  exit 1; \
	fi

# ------------------------------------------------------------------
# Image
# ------------------------------------------------------------------
.PHONY: sandbox-build
sandbox-build:
	$(CONTAINER_CLI) build \
	    --build-arg UID=$(HOST_UID) \
	    --build-arg GID=$(HOST_GID) \
	    -t $(IMAGE_NAME):$(IMAGE_TAG) \
	    $(CURDIR)

# ------------------------------------------------------------------
# Lifecycle
# ------------------------------------------------------------------
.PHONY: sandbox-up
sandbox-up: _require_env
	@$(CONTAINER_CLI) inspect $(CONTAINER) >/dev/null 2>&1 \
	    && echo "$(CONTAINER) already exists; use 'make sandbox-bash' or 'make sandbox-reset'." \
	    || $(CONTAINER_CLI) run -d \
	        --name $(CONTAINER) \
	        --hostname konf-sandbox \
	        --env-file $(ENV_FILE) \
	        -v $(REPO_ROOT):$(MOUNT_TARGET):rw \
	        -v $(VOL_CARGO):/home/konf-agent/.cargo \
	        -v $(VOL_NPM):/home/konf-agent/.npm \
	        $(RUN_EXTRA) \
	        $(IMAGE_NAME):$(IMAGE_TAG)

.PHONY: sandbox-bash
sandbox-bash: ## Drop into a bash shell inside the container (debugging only).
	$(CONTAINER_CLI) exec -it $(CONTAINER) bash

.PHONY: sandbox-mcp-test
sandbox-mcp-test: ## Smoke-test: spawn each MCP server once with --help / --version.
	@echo "== konf-mcp =="
	@$(CONTAINER_CLI) exec $(CONTAINER) konf-mcp --version 2>&1 || echo "(konf-mcp did not respond to --version; build may be pending)"
	@echo "== claude mcp serve =="
	@$(CONTAINER_CLI) exec $(CONTAINER) claude --version 2>&1 || echo "(claude cli missing?)"
	@echo "== github-mcp-server =="
	@$(CONTAINER_CLI) exec $(CONTAINER) github-mcp-server --version 2>&1 || echo "(github-mcp-server missing?)"

.PHONY: sandbox-down
sandbox-down:
	-$(CONTAINER_CLI) stop $(CONTAINER)
	-$(CONTAINER_CLI) rm   $(CONTAINER)

.PHONY: sandbox-reset
sandbox-reset: sandbox-down sandbox-build sandbox-up

.PHONY: sandbox-logs
sandbox-logs:
	$(CONTAINER_CLI) logs -f $(CONTAINER)

.PHONY: sandbox-status
sandbox-status:
	@$(CONTAINER_CLI) ps --filter name=$(CONTAINER) --format \
	    "table {{.Names}}\t{{.Status}}\t{{.Image}}"

# ------------------------------------------------------------------
# Deep clean — wipe caches (forces a full konf-mcp rebuild next up).
# ------------------------------------------------------------------
.PHONY: sandbox-clean
sandbox-clean: sandbox-down
	-$(CONTAINER_CLI) volume rm $(VOL_CARGO) $(VOL_NPM)
	-$(CONTAINER_CLI) rmi $(IMAGE_NAME):$(IMAGE_TAG)

.PHONY: help
help:
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z0-9_-]+:.*?## / {printf "  %-20s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
