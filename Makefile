.PHONY: build run lint format format-check test shell-check check-linux check hooks clean

build:
	bash scripts/build.sh

run: build
	./dist/devports

lint:
	cargo clippy --locked --all-targets -- -D warnings

format:
	cargo fmt

format-check:
	cargo fmt --check

test:
	cargo test --locked
	bash scripts/tests/installer.sh
	bash scripts/tests/release.sh

shell-check:
	@for script in scripts/*.sh scripts/tests/*.sh .githooks/*; do bash -n "$$script" || exit; done

check-linux: lint format-check shell-check
	cargo test --locked

check: lint format-check test build shell-check

hooks:
	@git config core.hooksPath .githooks
	@printf 'Git hooks enabled from .githooks.\n'

clean:
	cargo clean
	rm -rf dist release
