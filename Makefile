.DEFAULT_GOAL := help

.PHONY: help build format lint deps test check hooks tools site site-serve oracle-fetch oracle oracle-update

# The odoo/odoo commits the false-positive oracle scans, per series. A bump is reviewed through the
# diff that `make oracle-update` leaves in tests/oracle/.
SERIES ?= 18.0
ODOO_18.0 := 2ad2a6f07e0d35a1dfb800648d6e1c62dc712395
ODOO_20.0 := 020667cba52c9098143156d41dfcf9aa4667db76
ORACLE := .oracle/$(SERIES)

help: ## List available targets with their descriptions
	@grep -hE '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*## "} {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

build: ## Build the release binary
	cargo build --release --locked

format: ## Format the code (rustfmt)
	cargo fmt --all

lint: ## Lint every crate and target, warnings as errors (clippy)
	cargo clippy --workspace --all-targets --locked -- -D warnings

deps: ## Unused dependencies (cargo-machete), advisories, licenses and sources (cargo-deny)
	cargo machete
	cargo deny --locked check

test: ## Run the test suite, the fixture snapshot among them
	cargo test --workspace --locked

check: ## Everything CI runs, but the git hooks: formatting, lints, dependencies and tests
	cargo fmt --all --check
	$(MAKE) lint deps test

hooks: ## Install the git hooks, then run them over the whole tree (prek)
	prek install
	prek run --all-files

tools: ## Install the cargo tools `deps` runs
	cargo install --locked cargo-deny@0.20.2 cargo-machete@0.9.2

site: ## Build the documentation site into site/, failing on any warning or broken link
	uv sync --locked --only-group docs --no-install-project
	uv run --no-sync zensical build --clean --strict

site-serve: ## Serve the documentation site locally, rebuilding on change
	uv sync --locked --only-group docs --no-install-project
	uv run --no-sync zensical serve

oracle-fetch: ## Fetch the pinned Odoo core of SERIES (default 18.0) into .oracle/, without tests and assets but scripts and templates
	rm -rf $(ORACLE)
	git init -q $(ORACLE)
	git -C $(ORACLE) fetch -q --depth 1 --filter=blob:none https://github.com/odoo/odoo $(ODOO_$(SERIES))
	git -C $(ORACLE) sparse-checkout set --no-cone '/addons/' '/odoo/addons/' '!**/i18n/' '!**/static/**' \
		'**/static/src/**/*.js' '**/static/src/**/*.xml' '!**/tests/'
	git -C $(ORACLE) checkout -q FETCH_HEAD

# A fixed finding fails the gate as much as a new one, so the recorded list stays exact.
oracle: build ## Scan the fetched Odoo core of SERIES; fail unless the findings are those in tests/oracle/
	cp tests/oracle/$(SERIES).json $(ORACLE)/baseline.json
	target/release/oscn check $(ORACLE) --odoo-version $(SERIES) --baseline $(ORACLE)/baseline.json \
		2>$(ORACLE)/notices.txt || { cat $(ORACLE)/notices.txt; exit 1; }
	cat $(ORACLE)/notices.txt
	! grep -q 'are fixed' $(ORACLE)/notices.txt

oracle-update: build ## Record the findings of the fetched Odoo core of SERIES in tests/oracle/
	target/release/oscn check $(ORACLE) --odoo-version $(SERIES) --baseline $(ORACLE)/baseline.json --write-baseline
	cp $(ORACLE)/baseline.json tests/oracle/$(SERIES).json
