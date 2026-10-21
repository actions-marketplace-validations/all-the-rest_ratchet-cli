# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-21

The first release. This entry is written by hand — from `0.2.0` on, every
block is written by `ratchet release`.

### Added

- the gates: a count against a baseline that may only move in one declared
  direction
- the drift gates: the generated files, compared with their committed
  counterpart
- the findings registry: a list of findings becomes a number
- the Markdown report: a run rendered into one file
- the pre-release check: a command compared with its committed snapshot
- the GitHub Action: validate the gates, or move the baseline deliberately
- the self-update: the newest release, checksum verified

## [0.2.0] - 2026-10-21

### Added

- the release step — the version and the changelog from the history (release)

### Fixed

- the publish needs the repository, and the release can run again (release)
- the report says that it is generated — and how (report)

## [0.3.0] - 2026-10-21

### Added

- the drift gate creates the directory of the generated file (drift)

### Fixed

- the paths of a project may not leave the repository root (path)
