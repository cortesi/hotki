#!/bin/sh

cargo run --bin smoketest -- screenshots --theme default ./assets/default
cargo run --bin smoketest -- screenshots --theme solarized-dark ./assets/solarized-dark
cargo run --bin smoketest -- screenshots --theme solarized-light ./assets/solarized-light
cargo run --bin smoketest -- screenshots --theme dark-blue ./assets/dark-blue
cargo run --bin smoketest -- screenshots --theme charcoal ./assets/charcoal


