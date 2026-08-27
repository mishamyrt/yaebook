VERSION = 0.0.1

PREFIX = ${HOME}/.local/bin
TARGET = target/release/yaebook

all: $(TARGET)

$(TARGET): Cargo.toml Cargo.lock $(shell find crates -type f)
	cargo build --profile release

.PHONY: fmt
fmt:
	cargo fmt --all

.PHONY: lint
lint:
	cargo clippy --workspace --all-targets -- -D warnings

.PHONY: test
test:
	cargo test --workspace --all-targets

.PHONY: install
install: $(TARGET)
	cp $(TARGET) $(PREFIX)/yaebook

.PHONY: clean
clean:
	rm -rf target

.PHONY: publish
publish:
	@sed -E 's/^version = "[^"]+"/version = "${VERSION}"/' Cargo.toml > Cargo.toml.tmp
	@mv Cargo.toml.tmp Cargo.toml
	@cargo update -p yaebook
	@git add Makefile Cargo.toml Cargo.lock
	@git commit -m "chore: release ${VERSION} 🔥"
	@git tag "v${VERSION}"
	@git-cliff -o CHANGELOG.md
	@git tag -d "v${VERSION}"
	@git add CHANGELOG.md
	@git commit --amend --no-edit
	@git tag -a "v${VERSION}" -m "release v${VERSION}"
	@git push
	@git push --tags
