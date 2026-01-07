# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.8] - 2026-01-07

### Added
- Message log clearing in `wrap_reconnect` to prevent unbounded memory growth
- Comprehensive unit test suite (21 tests) for config, errors, and messages
- Documentation for callback safety and panic behavior
- Security documentation for `connect_script` and `application_name`
- Documentation for parameter cloning overhead in query methods

### Changed
- Improved `wrap_reconnect` documentation
- Reverted changes in 0.3.7

## [0.3.7] - 2025-09-06

### Changed
- Removed `Send + Sync` trait bounds on `PGError::Other` for more flexible error handling

## [0.3.6] - 2025-09-06

### Added
- Generic `Other` error variant to `PGError` enum via `PGError::other()` constructor
- Allows wrapping custom error types

## [0.3.5] - 2025-09-02

### Changed
- Modified how builder handles `with_xxx` functions for better ergonomics

## [0.3.4] - 2025-08-24

### Added
- `PGMessage::Connected` variant to indicate successful connection
- Optimized connect script execution

### Fixed
- Bug in connect script handling

## [0.3.3] - 2025-08-24

### Changed
- Documentation updates

## [0.3.2] - 2025-08-24

### Added
- `PGRobustClientConfig` builder for configuration

### Changed
- **Breaking**: Configuration is now done through `PGRobustClientConfig` builder instead of individual parameters

## [0.3.1] - 2025-08-15

### Changed
- Documentation improvements

## [0.3.0] - 2025-08-14

### Added
- `PGRobustClient` as the main client type
- Unified callback system for all message types (NOTIFY, RAISE, TIMEOUT, RECONNECT)
- Automatic reconnection with exponential backoff and jitter
- Query timeout support with server-side cancellation
- `PGMessage` enum for all event types

### Changed
- **Breaking**: Renamed `PGNotifyingClient` to `PGRobustClient`
- **Breaking**: Queries no longer require accessing inner client
- **Breaking**: Single callback handles all notification types

### Removed
- Separate handlers for different notification types

## [0.2.1] - 2025-05-21

### Added
- Repository badges in README

## [0.2.0] - 2025-08-10

### Added
- `PGRobustNotifier` for automatic reconnect after lost connection
- Basic reconnection support

## [0.1.0] - 2025-05-21

### Added
- Initial release
- PostgreSQL LISTEN/NOTIFY support
- Basic client wrapper around `tokio_postgres`
- RAISE message handling

[0.3.8]: https://github.com/macprog-guy/postgres-notify/compare/v0.3.7...HEAD
[0.3.7]: https://github.com/macprog-guy/postgres-notify/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/macprog-guy/postgres-notify/compare/0.3.4...v0.3.6
[0.3.5]: https://github.com/macprog-guy/postgres-notify/compare/0.3.4...0.3.5
[0.3.4]: https://github.com/macprog-guy/postgres-notify/compare/0.3.3...0.3.4
[0.3.3]: https://github.com/macprog-guy/postgres-notify/compare/0.3.2...0.3.3
[0.3.2]: https://github.com/macprog-guy/postgres-notify/releases/tag/0.3.2
[0.3.1]: https://github.com/macprog-guy/postgres-notify/releases/tag/0.3.1
[0.3.0]: https://github.com/macprog-guy/postgres-notify/releases/tag/0.3.0
[0.2.1]: https://github.com/macprog-guy/postgres-notify/releases/tag/v0.2.1
[0.2.0]: https://github.com/macprog-guy/postgres-notify/releases/tag/0.2.0
[0.1.0]: https://github.com/macprog-guy/postgres-notify/releases/tag/0.1.0
