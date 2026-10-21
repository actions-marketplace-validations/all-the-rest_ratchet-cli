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
