// Complete reference demonstrating hotki configuration options and action types.
// Each option and action type is shown at least once with minimal redundancy.

// Choose a built-in theme: default, charcoal, dark-blue, solarized-dark, solarized-light
theme("default");

// === Key Bindings ===

hotki.mode(|m, ctx| {
  // === Style overlay (HUD + notifications) ===
  m.style(#{
    hud: #{
      // Text shown at the right edge of HUD rows for nested modes
      tag_submenu: ">>",

      // Position: n, ne, e, se, s, sw, w, nw, center
      pos: ne,
      // Pixel offset from anchor (x: right, y: up)
      offset: #{ x: -20.0, y: -40.0 },

      // Font sizes
      font_size: 16.0, // Description text
      key_font_size: 14.0, // Key tokens
      tag_font_size: 12.0, // Mode tags

      // Font weights: light, regular, medium, semibold, bold, extrabold
      title_font_weight: medium,
      key_font_weight: semibold,
      tag_font_weight: bold,
      mod_font_weight: extrabold,

      // Colors (CSS names or hex)
      title_fg: "#ffffff",
      bg: "#1a1a1a",
      key_fg: "white",
      key_bg: "#2a2a2a",
      mod_fg: "#00ff00",
      mod_bg: "#3a3a3a",
      tag_fg: "yellow",

      // Window properties
      opacity: 0.95,
      radius: 12.0, // Window corner radius

      // Key box styling
      key_radius: 4.0, // Key box corner radius
      key_pad_x: 8.0, // Horizontal padding
      key_pad_y: 4.0, // Vertical padding
    },

    notify: #{
      // Window properties
      width: 400.0,
      pos: right, // left or right
      opacity: 0.9,
      timeout: 3.5,
      buffer: 4,
      radius: 8.0, // Notification corner radius

      // === Per-kind Notification Styles ===
      // Each supports all styling options
      info: #{
        bg: "#1e1e1e",
        title_fg: "#88ccff",
        body_fg: "#cccccc",
        title_font_size: 14.0,
        title_font_weight: bold,
        body_font_size: 12.0,
        body_font_weight: regular,
        icon: "\uEA74",
      },

      warn: #{
        bg: "#332200",
        title_fg: "#ffaa00",
        body_fg: "#ffcc66",
        title_font_size: 14.0,
        title_font_weight: semibold,
        body_font_size: 12.0,
        body_font_weight: light,
        icon: "\uEA6C",
      },

      error: #{
        bg: "#330000",
        title_fg: "#ff4444",
        body_fg: "#ff8888",
        title_font_size: 15.0,
        title_font_weight: extrabold,
        body_font_size: 12.0,
        body_font_weight: medium,
        icon: "\uEA87",
      },

      success: #{
        bg: "#003300",
        title_fg: "#00ff00",
        body_fg: "#88ff88",
        title_font_size: 14.0,
        title_font_weight: medium,
        body_font_size: 12.0,
        body_font_weight: regular,
        icon: "\uF4B2",
      },
    },
  });

  // Global navigation keys (active across all modes while the HUD is visible).
  if ctx.hud {
    m.bind("esc", "Back", action.pop).global().hidden();
  }

  // Main modal entry point.
  m.mode("shift+cmd+0", "Main", |m, ctx| {

    // Exit (override the parent mode entry chord).
    m.bind("shift+cmd+0", "Exit", action.exit).global().hidden();

    // === Action Types (each shown at least once) ===

    // action.shell: execute a shell command.
    m.bind("s", "Shell Command", action.shell("echo 'Hello'"));

    // action.shell with notifications: show success/error notifications.
    m.bind(
      "shift+S",
      "Shell with Notifications",
      action.shell("echo 'Success'").notify(success, error),
    );

    // action.shell.silent: suppress all notifications.
    m.bind("shift+Q", "Shell Silent", action.shell("echo 'Quiet'").silent());

    // action.relay: send keystrokes to the active application.
    m.bind("r", "Relay Keystroke", action.relay("cmd+c"));

    // action.clear_notifications: remove all on-screen notifications.
    m.bind("c", "Clear Notifications", action.clear_notifications);

    // action.show_details: control details window: on/off/toggle.
    m.bind("d", "Toggle Details", action.show_details(toggle)).stay();

    // action.reload_config: reload the configuration file.
    m.bind("shift+R", "Reload Config", action.reload_config);

    // action.show_root: clear to root and show the HUD.
    m.bind("h", "Show Root HUD", action.show_root);

    // action.hide_hud: hide HUD while keeping the current stack active.
    m.bind("shift+H", "Hide HUD", action.hide_hud);

    // action.theme_*: change theme (stay in the mode for repeated changes).
    m.bind([
      ["n", "Next Theme", action.theme_next],
      ["p", "Previous Theme", action.theme_prev],
      ["1", "Dark Blue Theme", action.theme_set("dark-blue")],
      ["2", "Charcoal Theme", action.theme_set("charcoal")],
      ["3", "Default Theme", action.theme_set("default")],
    ]).stay();

    // action.set_volume: set system volume to absolute value (0-100).
    m.bind("v", "Set Volume 50%", action.set_volume(50)).stay();

    // action.change_volume: adjust volume by relative amount (-100 to +100).
    m.bind("shift+V", "Change Volume +10", action.change_volume(10)).stay();

    // action.mute: control mute: on/off/toggle.
    m.bind([
      ["m", "Mute", action.mute(on)],
      ["u", "Unmute", action.mute(off)],
      ["shift+M", "Toggle Mute", action.mute(toggle)],
    ]).stay();

    // === Handler ===
    m.bind("x", "Complex Action", handler(|ctx| {
      ctx.exec(action.shell("echo hello").silent());
      ctx.notify(success, "Handler", "Queued a shell command + notification");
      ctx.stay();
    }));

    // === Binding modifiers ===
    m.bind("g", "Global Key", action.shell("echo 'Global'")).global();
    m.bind("shift+X", "Hidden Key", action.shell("echo 'Hidden'")).hidden();

    // "HUD-only" bindings are just conditional renders.
    if ctx.hud {
      m.bind("o", "HUD Only", action.shell("echo 'HUD Only'"));
    }

    // Hold-to-repeat: repeat settings imply `.stay()`.
    m.bind("a", "Repeat Shell (custom timings)", action.shell("echo tick"))
      .repeat_ms(250, 350);
    m.bind("b", "Repeat Relay (defaults)", action.relay("cmd+c"))
      .repeat();

    // === Mode navigation ===
    m.mode("shift+K", "Nested Mode", |sub, ctx| {
      sub.style(#{ hud: #{ mode: mini } });
      sub.bind([
        ["b", "Back One Level", action.pop],
        ["e", "Exit All", action.exit],
      ]);
    });

    // `.capture()` on a mode entry: capture all keys while HUD is visible.
    m.mode("shift+C", "Capture Mode", |sub, ctx| {
      sub.bind("b", "Back One Level", action.pop);
    }).capture();

    // === Context matching patterns (conditional bindings) ===
    if ctx.app.matches("Safari|Chrome") {
      m.mode("shift+B", "Browser Mode", |sub, ctx| {
        sub.bind("r", "Reload", action.relay("cmd+r"));
      });
    }

    if ctx.title.matches(".*\\.md$") {
      m.mode("shift+ctrl+t", "Title Match Mode", |sub, ctx| {
        sub.bind("s", "Save", action.relay("cmd+s"));
      });
    }
  });
});
