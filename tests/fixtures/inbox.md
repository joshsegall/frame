# Inbox

- Parser crashes on empty effect block #bug
  Saw this when testing with empty `handle {}` blocks.
  Stack trace points to parser/effect.rs line 142.

- Think about whether `perform` should be an expression or statement
  #design
  If it's an expression, we get composability:
  ```lace
  let x = perform Ask() + 1
  ```
  But it makes the effect type more complex.

- CC found bug in module resolution for re-exported effect types
  #cc-added #bug

- Read the Koka paper on named handlers #research

- Unique type inference interacts with effect handlers somehow
  #research
  Noticed this while working on EFF-014, not sure of implications
  yet — could be a big deal.