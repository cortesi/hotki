// Complete reference demonstrating all hotki configuration options and action types.
// Each option and action type is shown exactly once with minimal redundancy.

// Choose a built-in theme: default, charcoal, dark_blue, solarized_dark, solarized_light
base_theme("default");

// === Style overlay (HUD + notifications) ===
style(#{
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

// === Key Bindings ===

// Main modal entry point
global.mode("shift+cmd+0", "Main", |m| {

  // === Action Types (each shown once) ===
  // action.shell: Execute a shell command
  m.bind("s", "Shell Command", action.shell("echo 'Hello'"));

  // action.shell with notifications: Execute command and show success/error notifications
  m.bind(
    "shift+S",
    "Shell with Notifications",
    action.shell("echo 'Success'").notify(success, error),
  );

  // action.relay: Send keystrokes to the active application
  m.bind("r", "Relay Keystroke", action.relay("cmd+c"));

  // action.clear_notifications: Remove all on-screen notifications
  m.bind("c", "Clear Notifications", action.clear_notifications);

  // action.show_details: Control details window: on/off/toggle
  m.bind("d", "Toggle Details", action.show_details(toggle));

  // action.reload_config: Reload the configuration file
  m.bind("shift+R", "Reload Config", action.reload_config);

  // action.show_hud_root: Display the root-level HUD
  m.bind("h", "Show HUD Root", action.show_hud_root);

  // action.theme_next: Switch to the next theme
  m.bind("n", "Next Theme", action.theme_next);

  // action.theme_prev: Switch to the previous theme
  m.bind("p", "Previous Theme", action.theme_prev);

  // action.theme_set: Set a specific theme by name
  m.bind("1", "Dark Blue Theme", action.theme_set("dark-blue"));
  m.bind("2", "Charcoal Theme", action.theme_set("charcoal"));
  m.bind("3", "Default Theme", action.theme_set("default"));

  // action.set_volume: Set system volume to absolute value (0-100)
  m.bind("v", "Set Volume 50%", action.set_volume(50));

  // action.change_volume: Adjust volume by relative amount (-100 to +100)
  m.bind("shift+V", "Change Volume +10", action.change_volume(10));

  // action.mute: Control mute: on/off/toggle
  m.bind("m", "Mute", action.mute(on));
  m.bind("u", "Unmute", action.mute(off));
  m.bind("shift+M", "Toggle Mute", action.mute(toggle));

  // === User UI Control ===
  // action.user_style: Control user style configuration (HUD and notifications): on/off/toggle
  m.bind("t", "Toggle User UI", action.user_style(toggle));
  m.bind("shift+T", "Disable User UI", action.user_style(off));
  m.bind("shift+U", "Enable User UI", action.user_style(on));

  // === Mode Navigation ===
  // mode: Enter a nested mode with its own key bindings
  m.mode("shift+K", "Nested Mode", |sub| {
    // action.pop: Return to the previous mode
    sub.bind("b", "Back One Level", action.pop);
    // action.exit: Exit all modes and return to root
    sub.bind("e", "Exit All", action.exit);
  })
  .style(#{ hud: #{ mode: hud_mini } });

  // === Mode Attributes ===
  m.bind(
    "a",
    "No Exit + Repeat (custom timings)",
    action.shell("echo 'Stay'"),
  )
  .no_exit()
  .repeat_ms(250, 350);

  m.bind("g", "Global Key", action.shell("echo 'Global'")).global();
  m.bind("shift+H", "Hidden Key", action.shell("echo 'Hidden'")).hidden();
  m.bind("o", "HUD Only", action.shell("echo 'HUD Only'")).hud_only();

  // capture: Swallow all non-bound keys while HUD is visible for this mode.
  // Only keys explicitly bound in the current mode (including globals) are handled.
  m.mode("shift+C", "Capture Mode", |sub| {
    sub.bind("b", "Back One Level", action.pop);
  }).capture();

  // === App/Title Matching ===
  m.mode("b", "Browser Mode", |sub| {
    sub.bind("r", "Reload", action.relay("cmd+r"));
  }).match_app("Safari|Chrome");

  m.mode("shift+ctrl+t", "Title Match Mode", |sub| {
    sub.bind("s", "Save", action.relay("cmd+s"));
  }).match_title(".*\\.md$");

  // Global navigation (combined attributes)
  m.bind("esc", "Back", action.pop).global().hidden();
});
