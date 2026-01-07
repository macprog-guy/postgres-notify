test-no-capture:
	cargo insta test --all-targets -- --no-capture

test:
	cargo insta test --all-targets

review:
	cargo insta review

build:
	cargo build

release:
	cargo build --release

publish:
	cargo publish

audit:
	cargo audit --ignore RUSTSEC-2025-0111 --ignore RUSTSEC-2025-0134Stre