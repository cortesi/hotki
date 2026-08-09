# Repository Instructions

<!-- nanocode:standards:start -->
## Shared coding standards

- For tasks that change code or developer tooling, load and follow the
  `nano-code` skill.
- For Rust code, Cargo manifests, build scripts, tests, or validation, also load
  and follow the `nano-rust` skill.
- Repository-specific instructions in this file override those skill defaults.
<!-- nanocode:standards:end -->

## Platform And Validation

- Hotki is a macOS-only hotkey application.
- Do not add conditional compilation for other platforms.
- Unit tests and smoke tests can take more than 60 seconds.
- Use an extended external timeout as a safety bound for those commands.
- After a user-interface change, run `cargo run --locked --bin smoketest -- all`.
