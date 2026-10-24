## Summary

<!-- Briefly describe what this pull request changes. -->

## Motivation

<!-- Why is this change needed? Link the relevant issue if one exists. -->

Closes #

## Technical approach

<!-- Describe the approach taken and any alternatives considered. -->

## Unsafe-code review

<!-- List any new or modified `unsafe` blocks and summarize their SAFETY
     justification. Write "None" if this change adds no unsafe code. -->

## Testing performed

<!-- What commands did you run? What QEMU/serial output did you observe? -->

## Documentation changes

<!-- Which docs were updated (README, ARCHITECTURE.md, docs/, CHANGELOG.md)?
     Write "None" if no documentation changes were needed. -->

## Compatibility impact

<!-- Does this change affect boot behavior, visible framebuffer output,
     dependency versions, or the public module layout? -->

## Checklist

* [ ] `cargo fmt --all -- --check` passes
* [ ] `cargo build` succeeds
* [ ] Boot tested with `cargo run` (when relevant)
* [ ] Fault test run with `cargo run --features fault-tests` (when relevant)
* [ ] Graphical boot logo verified by maintainer (cannot be automated in CI)
* [ ] Every new `unsafe` block has a `// SAFETY:` comment
* [ ] No unrelated changes are included
* [ ] Documentation is updated (when relevant)
* [ ] `CHANGELOG.md` is updated (when relevant)
* [ ] No secrets or generated build artifacts are committed
* [ ] No exploit details for an unreleased vulnerability are included
      (use private vulnerability reporting; see `SECURITY.md`)
