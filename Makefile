tests:
	cargo insta test --all-targets

review:
	cargo insta review

build:
	cargo build

release:
	cargo build --release

publish:
	cargo publish