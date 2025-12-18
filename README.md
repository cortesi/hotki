![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

<p align="center">
  <img src="crates/hotki/assets/logo-doc.png" alt="hotki logo" width="200" />
</p>

# Hotki

A modal hotkey app for macOS.

- Modal hotkeys for macOS
- A customizable HUD (Heads-Up Display) for displaying active mode hotkeys
- Customizable notifications to display hotkey action outcomes
- Hotkeys for any app with key relaying and focus matching

Hotki is now an early alpha - it's stable and my daily driver, but I'm not
cutting binary releases yet. See the [Installation](#installation) section
below for how to build it. Next steps:

- External window-management CLI (Hotki will call it via `shell(...)` bindings)
- More sophisticated HUD patterns allowing text entry, selection, etc.
- Window groups


## Configuration

Hotki configuration lives at `~/.hotki/config.rhai` and is written in Rhai.

- [Full reference](CONFIG.md)
- Examples: `examples/complete.rhai`, `examples/cortesi.rhai`, `examples/match.rhai`, `examples/test.rhai`

Validate a config without starting the UI:

```bash
hotki check --config ~/.hotki/config.rhai
hotki check  # uses the default resolution policy
```

Minimal example:

```rhai
theme("default");

hotki.mode(|m, ctx| {
  m.style(#{
    hud: #{
      pos: ne,
      mode: hud,
    },
  });

  if ctx.hud {
    m.bind("esc", "Back", action.pop).global().hidden();
  }

  m.mode("shift+cmd+0", "Main", |m, ctx| {
    m.bind("s", "Save", action.relay("cmd+s")).stay();
    m.bind("n", "Next Theme", action.theme_next).stay();
    m.bind("p", "Previous Theme", action.theme_prev).stay();
    m.bind("shift+cmd+0", "Exit", action.exit).global().hidden();
  });
});
```

## Themes and Styling

Every aspect of Hotki's UI is customizable. We have a few built-in
[themes](./themes) that you can build on (embedded into the binary at compile
time). To customize, copy one into your config directory's `themes/` folder
(usually `~/.hotki/themes/`) and tweak it.


<table>
  <tr>
    <td> 
        <center><b>default</b></center>
        <img src="./assets/default/001_hud.png" width="350px">
    </td>
    <td> 
        <img src="./assets/default/003_notify_info.png" width="250px">
        <img src="./assets/default/002_notify_success.png" width="250px">
        <img src="./assets/default/004_notify_warning.png" width="250px">
        <img src="./assets/default/005_notify_error.png"width="250px">
    </td>
  </tr>
  <tr></tr>
  <tr>
    <td> 
        <center><b>solarized-dark</b></center>
        <img src="./assets/solarized-dark/001_hud.png" width="350px">
    </td>
    <td> 
        <img src="./assets/solarized-dark/003_notify_info.png" width="250px">
        <img src="./assets/solarized-dark/002_notify_success.png" width="250px">
        <img src="./assets/solarized-dark/004_notify_warning.png" width="250px">
        <img src="./assets/solarized-dark/005_notify_error.png"width="250px">
    </td>
  </tr>
  <tr></tr>
  <tr>
    <td>
        <center><b>solarized-light</b></center>
        <img src="./assets/solarized-light/001_hud.png" width="350px">
    </td>
    <td> 
        <img src="./assets/solarized-light/003_notify_info.png" width="250px">
        <img src="./assets/solarized-light/002_notify_success.png" width="250px">
        <img src="./assets/solarized-light/004_notify_warning.png" width="250px">
        <img src="./assets/solarized-light/005_notify_error.png"width="250px">
    </td>
  </tr>
  <tr></tr>
  <tr>
    <td>
        <center><b>dark-blue</b></center>
        <img src="./assets/dark-blue/001_hud.png" width="350px">
    </td>
    <td> 
        <img src="./assets/dark-blue/003_notify_info.png" width="250px">
        <img src="./assets/dark-blue/002_notify_success.png" width="250px">
        <img src="./assets/dark-blue/004_notify_warning.png" width="250px">
        <img src="./assets/dark-blue/005_notify_error.png"width="250px">
    </td>
  </tr>
  <tr></tr>
  <tr>
    <td>
        <center><b>charcoal</b></center>
        <img src="./assets/charcoal/001_hud.png" width="350px">
    </td>
    <td> 
        <img src="./assets/charcoal/003_notify_info.png" width="250px">
        <img src="./assets/charcoal/002_notify_success.png" width="250px">
        <img src="./assets/charcoal/004_notify_warning.png" width="250px">
        <img src="./assets/charcoal/005_notify_error.png"width="250px">
    </td>
  </tr>
</table>


## Fonts

The default bundled font is a [Nerd Font](https://www.nerdfonts.com/)
([0xProto](https://github.com/0xType/0xProto)
Nerd Font Mono). Nerd Fonts include a wide range of glyphs and symbols used
throughout the UI, and which can be used in styling.


# Installation

We don't have binary releases yet. For the moment, the installation process is
to compile the app bundle from the repo root:

```sh
cargo xtask bundle
```

The bundle will be at `./target/bundle/Hotki.app`, ready to copy to your
`/Applications` folder.
