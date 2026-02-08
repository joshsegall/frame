# Effect System

> Design and implement the algebraic effect system for Lace.

## Backlog

- [>] `EFF-014` Implement effect inference for closures #core
  - added: 2025-05-10
  - dep: EFF-003
  - spec: doc/spec/effects.md#closure-effects
  - ref: doc/design/effect-handlers-v2.md
  - note:
    Found while working on EFF-002.

    The desugaring needs to handle three cases:
    1. Simple perform with no resumption
    2. Perform with single-shot resumption
    3. Perform with multi-shot resumption (if we support it)

    See the Koka paper for approach:
    ```lace
    handle(e) { ... } with {
      op(x, resume) -> resume(x + 1)
    }
    // desugars to match on effect tag
    ```
  - [ ] `EFF-014.1` Add effect variables to closure types
    - added: 2025-05-10
  - [>] `EFF-014.2` Unify effect rows during inference #cc
    - added: 2025-05-11
    - note: Row unification is the hard part here
    - [ ] `EFF-014.2.1` Handle row polymorphism
    - [ ] `EFF-014.2.2` Implement row simplification
  - [ ] `EFF-014.3` Test with nested closures
- [ ] `EFF-015` Effect handler optimization pass #core
  - dep: EFF-014
- [-] `EFF-012` Effect-aware dead code elimination #core
  - dep: EFF-014, INFRA-003
- [ ] `EFF-016` Error messages for effect mismatches #core
- [ ] `EFF-017` Research: algebraic effect composition #research
- [ ] `EFF-018` Design doc: effect aliases #design

## Parked

- [~] `EFF-020` Higher-order effect handlers #research

## Done

- [x] `EFF-003` Implement effect handler desugaring #core
  - resolved: 2025-05-14
- [x] `EFF-002` Parse effect declarations #core
  - resolved: 2025-05-12
- [x] `EFF-001` Define effect syntax #core
  - resolved: 2025-05-08