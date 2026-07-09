<!--
Sync Impact Report
==================
Version change: 2.0.1 → 2.0.2
Rationale: PATCH — TODO(GUI_FRAMEWORK) resolved: Tauri 2.x + Svelte, pinned by the
001-phone-webcam-bridge plan as the constitution required. No principle changes.

Previous (2.0.0 → 2.0.1): PATCH — product renamed DroidCamLink → CamLink (decision
recorded in specs/001-phone-webcam-bridge/spec.md, Clarifications 2026-07-09).
Linux+Windows parity (Principle IV) was reaffirmed in the same session.

Previous (1.0.0 → 2.0.0): MAJOR — Test-First made mandatory, stack pinned to Rust,
Cross-Platform Parity added.

Modified principles:
- IV. Testing Discipline and Measurable Success → III. Test-First (NON-NEGOTIABLE)
- V. Quality Gates and Observability → VI. Quality Gates and Observability (renumbered)
- III. Simplicity and YAGNI → V. Simplicity and YAGNI (renumbered)

Added sections:
- Principle IV: Cross-Platform Parity (Linux + Windows)
- Principle (part of IV/Additional Constraints): Rust toolchain and quality rules

Removed sections: none

Templates status:
- .specify/templates/tasks-template.md ✅ updated (test tasks changed from OPTIONAL
  to REQUIRED, per Principle III)
- .specify/templates/plan-template.md ✅ aligned (Constitution Check gate covers new
  principles; Technical Context now expected to default to the pinned Rust stack)
- .specify/templates/spec-template.md ✅ aligned (no changes needed; success criteria
  remain technology-agnostic)
- .specify/templates/checklist-template.md ✅ no constitution references
- .specify/templates/commands/*.md — directory not present; speckit commands are
  Claude skills under .claude/skills/ and load this file dynamically

Follow-up TODOs: none.

Resolved: TODO(PROJECT_NAME) — CamLink (v2.0.1); TODO(GUI_FRAMEWORK) — Tauri 2.x
+ Svelte, pinned by the 001-phone-webcam-bridge plan (v2.0.2).
-->

# CamLink Constitution

CamLink (formerly DroidCamLink) is a desktop application for Linux and Windows,
written in Rust, that turns Android phones (via USB) and IP/RTSP cameras into
virtual webcams.

## Core Principles

### I. Spec-First Development

Every feature MUST begin as a written specification under `specs/` before any
implementation work starts. The required flow is: specify → (clarify if needed) →
plan → tasks → implement. Specifications MUST describe user value and behavior in
technology-agnostic terms; unresolved ambiguities MUST be marked
`[NEEDS CLARIFICATION]` and resolved before planning completes.

**Rationale**: Writing the spec first keeps decisions reviewable, prevents scope
drift, and makes every later artifact (plan, tasks, code) traceable to a stated need.

### II. Independent, Incremental Delivery

User stories MUST be prioritized (P1, P2, P3, …) and each MUST be independently
implementable, testable, and demonstrable. The P1 story alone MUST constitute a
viable MVP. Work proceeds in priority order, and each completed story MUST leave the
system in a working, shippable state on both target platforms.

**Rationale**: Independent slices allow early validation, parallel work, and the
option to stop at any checkpoint with delivered value instead of a half-built whole.

### III. Test-First (NON-NEGOTIABLE)

TDD is mandatory for every feature. Test tasks MUST be generated for every user
story, written before the implementation, and observed to fail before the
implementation is written (Red → Green → Refactor). Coverage requirements:

- Unit tests for all non-trivial logic (`cargo test`, colocated `#[cfg(test)]`
  modules).
- Integration tests under `tests/` for every public contract and inter-component
  boundary (device communication, IPC, network protocol, persistence).
- Every spec MUST define measurable, technology-agnostic success criteria, and at
  least one automated test MUST map to each functional requirement.
- A story is complete only when its full test suite passes on Linux AND Windows.

**Rationale**: Failing-first tests are the only objective evidence that the system
does what the spec promised; on a two-platform target, automated tests are the only
scalable way to prevent silent regressions on the platform not in front of the
developer.

### IV. Cross-Platform Parity (Linux + Windows)

Every feature MUST work equivalently on Linux and Windows unless the spec explicitly
scopes it to one platform with justification. Platform-specific code MUST be
isolated behind a common abstraction (traits + `#[cfg(target_os = "...")]`
implementations in dedicated modules); business logic MUST remain platform-neutral.
CI MUST build and run the test suite on both platforms; a change that breaks either
platform MUST NOT be merged.

**Rationale**: Parity enforced from day one is cheap; retrofitting a second platform
onto platform-entangled code is a rewrite.

### V. Simplicity and YAGNI

Choose the simplest design that satisfies the current specification. New crates,
layers, patterns, or dependencies beyond what the spec requires MUST be justified in
the plan's Complexity Tracking table, including why the simpler alternative was
rejected. Speculative generality ("we might need it later") is not a valid
justification.

**Rationale**: Complexity is the main long-term cost driver; forcing an explicit,
reviewable justification keeps the codebase proportional to the problem.

### VI. Quality Gates and Observability

The plan-stage Constitution Check gate MUST pass before research (Phase 0) begins
and MUST be re-checked after design (Phase 1). Implemented code MUST include
deliberate error handling (`Result`-based, no `unwrap()`/`expect()` on fallible
paths outside tests) and structured logging (`tracing` or equivalent) on primary
paths so failures are diagnosable from output alone. A task is only done when the
change works end-to-end on both platforms, not merely when it compiles.

**Rationale**: Gates catch principle violations while they are cheap to fix, and
observable behavior turns field issues from guesswork into lookup.

## Additional Constraints

- **Language**: Rust, latest stable toolchain, managed via `rustup` and pinned with
  `rust-toolchain.toml`. Edition 2021 or later.
- **Targets**: `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc` are the
  supported release targets. Additional targets require a constitution amendment or
  explicit plan-level justification.
- **Lint/format gates**: `cargo fmt --check` and `cargo clippy -- -D warnings` MUST
  pass before any task is considered done.
- **Unsafe code**: `unsafe` blocks are forbidden unless required for FFI/OS APIs;
  each block MUST carry a `// SAFETY:` comment explaining the invariant.
- **GUI/desktop framework**: Tauri 2.x with a Svelte frontend (pinned by
  specs/001-phone-webcam-bridge/plan.md, 2026-07-09). Later features MUST reuse
  this decision unless a plan documents the reason to diverge.
- **Dependencies**: prefer the standard library and well-maintained crates; every
  new dependency is subject to Principle V justification.
- Secrets, credentials, and user-private data MUST NOT be committed to the
  repository or embedded in specs, plans, or tasks.

## Development Workflow & Quality Gates

- Artifacts live under `specs/[###-feature-name]/`: `spec.md`, `plan.md`,
  `tasks.md`, plus research/design documents as generated.
- Sequence per feature: `/speckit-specify` → `/speckit-clarify` (when ambiguity
  exists) → `/speckit-plan` → `/speckit-tasks` → `/speckit-implement`.
- Tasks MUST be organized by user story with explicit dependencies and exact file
  paths; every story MUST include its test tasks before its implementation tasks
  (Principle III).
- Every checkpoint in tasks.md is a validation point: stop, run
  `cargo fmt --check && cargo clippy -- -D warnings && cargo test`, verify the story
  works, then proceed. Failing verification blocks progression to the next story.
- CI runs the full gate (fmt, clippy, tests) on Linux and Windows for every change.

## Governance

This constitution supersedes ad-hoc practices for all work in this repository.

- **Amendments**: Any change to this document MUST state what changed and why, bump
  the version per the policy below, update the Sync Impact Report, and propagate
  required changes into the dependent templates under `.specify/templates/`.
- **Versioning policy**: Semantic versioning. MAJOR for removals or backward-
  incompatible redefinitions of principles/governance; MINOR for new principles or
  materially expanded guidance; PATCH for clarifications and wording fixes.
- **Compliance review**: The Constitution Check section of every feature plan is the
  enforcement point. Violations MUST either be corrected or justified in the
  Complexity Tracking table before implementation begins.

**Version**: 2.0.2 | **Ratified**: 2026-07-09 | **Last Amended**: 2026-07-09
