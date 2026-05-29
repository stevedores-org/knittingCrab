# Sovereign Intelligence Standards — knittingCrab

## 1. Architectural Integrity
- All new crates must be added to the root `Cargo.toml` workspace.
- Maintain strict trait boundaries to allow for local/remote execution polymorphism.
- Ensure the DAG logic in `knitting-crab-scheduler` remains cycle-free.

## 2. Distributed Safety
- Remote sessions must be named deterministically: `aivcs__{repo}__{work_id}__{role}`.
- Sanitization of all shell-bound parameters is non-negotiable.
- Lease heartbeats must be resilient to network jitter.

## 3. Operational Excellence
- `nix flake check` is the ultimate source of truth for CI status.
- All binary builds must be reproducible.
- Documentation (7-File Rule) must be updated for every major architectural change.
