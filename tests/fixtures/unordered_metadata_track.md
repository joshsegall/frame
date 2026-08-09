# Legacy Track

> Fields in the order a project accumulated them, before the canonical order
> existed. Nothing here is malformed — the parser accepts any order — so a write
> that does not touch these tasks must return them byte for byte.

## Backlog

- [ ] `LEG-001` Reference added after the note #core
  - added: 2025-05-01
  - note:
    `fr ref add` appends when the task has no `ref:` yet, so it lands here.
  - ref: src/legacy.rs
- [ ] `LEG-002` Spec before the deps it explains
  - added: 2025-05-02
  - spec: doc/legacy.md#anchor
  - dep: LEG-001

## Parked

- [~] `LEG-003` Parked with everything out of order
  - note: shelved until the parser lands
  - added: 2025-05-03
  - ref: src/parked.rs

## Done

- [x] `LEG-004` The shape this all came from
  - added: 2025-04-01
  - note:
    A note long enough that anything below it is past the fold.

    `set_state` appends `resolved:` on completion, so the date lands under
    the whole note and reads as though the task never had one.
  - resolved: 2025-04-25
