// Minimal example showing app/title matchers on Mode entries.
// Modes with matchers are visible/actionable only if the active app/title matches.

theme("default");

hotki.mode(|m, ctx| {
  m.style(#{
    hud: #{
      pos: ne,
      offset: #{ x: 0.0, y: 0.0 },
      font_size: 16.0,
      key_font_size: 16.0,
      title_font_weight: regular,
      key_font_weight: regular,
      tag_font_size: 16.0,
      tag_font_weight: regular,
      title_fg: "white",
      key_fg: "white",
      key_bg: "#303030",
      mod_fg: "white",
      mod_bg: "#404040",
      tag_fg: "white",
      bg: "#202020",
      opacity: 0.95,
    },
    notify: #{
      width: 420.0,
      pos: right,
      opacity: 0.95,
      timeout: 4.0,
      buffer: 5,
      info: #{
        bg: "#222222",
        title_fg: "white",
        body_fg: "white",
      },
      warn: #{
        bg: "#442a00",
        title_fg: "#ffc100",
        body_fg: "#ffc100",
      },
      error: #{
        bg: "#3a0000",
        title_fg: "#ff6666",
        body_fg: "#ff6666",
      },
      success: #{
        bg: "#0c2d0c",
        title_fg: "#8bff8b",
        body_fg: "#8bff8b",
      },
    },
  });

  m.mode("shift+cmd+0", "Activate", |m, ctx| {
    // App matching is just conditional rendering.
    if ctx.app.matches("Safari|Arc|Google Chrome") {
      m.mode("b", "Browser", |sub, ctx| {
        sub.bind("r", "Reload Tab", action.relay("cmd+r"));
        sub.bind("t", "New Tab", action.relay("cmd+t"));
        sub.bind("p", "Back", action.pop);
      });
    }

    // Title matching is also a conditional.
    if ctx.title.matches(".*(?i:readme).*") {
      m.mode("r", "Reader", |sub, ctx| {
        sub.bind("m", "Mark Read", action.shell("echo marked"));
        sub.bind("p", "Back", action.pop);
      });
    }

    // Normal always-visible action.
    m.bind("h", "Hello", action.shell("echo hello"));

    // Global navigation keys (hidden from the HUD).
    m.bind("shift+cmd+0", "Exit", action.exit).global().hidden();
    m.bind("esc", "Back", action.pop).global().hidden();
  });
});
