// A rough faximile of cortesi's yabai + wezterm + spotify config
//
// This config assumes:
// - `yabai` is installed and running
// - `spotify` is available on PATH (or adjust commands)
// - `/opt/homebrew/bin/wezterm` exists (or adjust)

theme("default");

fn yabai_grid(cols, rows, x, y, w, h) {
  action.shell(`yabai -m window --grid ${rows}:${cols}:${x}:${y}:${w}:${h}`)
}

fn yabai_cell(cols, rows, x, y) {
  yabai_grid(cols, rows, x, y, 1, 1)
}

fn yabai_cell_move(cols, rows, dx, dy) {
  action.shell(
    `
      osascript -l JavaScript \
        -e 'const app = Application.currentApplication(); app.includeStandardAdditions = true;' \
        -e 'const cols = ${cols}; const rows = ${rows}; const dx = ${dx}; const dy = ${dy};' \
        -e 'function query(args) { return JSON.parse(app.doShellScript("yabai -m query " + args)); }' \
        -e 'const win = query("--windows --window"); const disp = query("--displays --display");' \
        -e 'const wf = win.frame; const df = disp.frame;' \
        -e 'const wx = wf.x, wy = wf.y, ww = wf.w, wh = wf.h;' \
        -e 'const ox = df.x, oy = df.y, dw = df.w, dh = df.h;' \
        -e 'const cellW = dw / cols; const cellH = dh / rows;' \
        -e 'let x = Math.round((wx - ox) / cellW); let y = Math.round((wy - oy) / cellH);' \
        -e 'let w = Math.max(1, Math.round(ww / cellW)); let h = Math.max(1, Math.round(wh / cellH));' \
        -e 'x = Math.min(Math.max(x + dx, 0), cols - w); y = Math.min(Math.max(y + dy, 0), rows - h);' \
        -e 'const spec = rows + ":" + cols + ":" + x + ":" + y + ":" + w + ":" + h; app.doShellScript("yabai -m window --grid " + spec);'
    `
  )
}

hotki.mode(|m, ctx| {
  // Always-on space switching (hidden from HUD, active in all modes).
  for i in 1..=9 {
    m.bind(`cmd+${i}`, `${i}`, action.shell(`yabai -m space --focus ${i}`))
      .global()
      .hidden();
  }
  m.bind("cmd+0", "10", action.shell("yabai -m space --focus 10"))
    .global()
    .hidden();

  m.bind(
    "cmd+return",
    "wezterm window",
    action.shell("/opt/homebrew/bin/wezterm cli spawn --new-window")
  )
    .global()
    .hidden();

  m.bind([
    ["shift+cmd+y", "y scratchpad", action.shell("yabai -m window --toggle web")],
    ["shift+cmd+u", "u scratchpad", action.shell("yabai -m window --toggle web2")],
  ]).global().hidden();

  // Global navigation keys (hidden from HUD).
  if ctx.hud {
    m.bind([
      ["esc", "Exit", action.exit],
      ["left", "Pop", action.pop],
    ]).global().hidden();
  }

  m.mode("shift+cmd+m", "music", |m, ctx| {
    m.bind([
      ["k", "vol up", action.change_volume(5)],
      ["j", "vol down", action.change_volume(-5)],
    ]).repeat();

    m.bind([
      ["l", "next >>", action.shell("spotify next")],
      ["h", "<< prev", action.shell("spotify prev")],
      ["p", "play/pause", action.shell("spotify pause")],
      ["m", "mute", action.mute(toggle)],
    ]).stay();

    m.bind("shift+cmd+m", "exit", action.exit).global().hidden();
  })
    .capture();

  m.mode("shift+cmd+space", "activate", |m, ctx| {
    m.mode("h", "hotki", |sub, ctx| {
      sub.bind([
        ["c", "Clear Notifications", action.clear_notifications],
        ["d", "Details", action.show_details(toggle)],
        ["r", "Reload Config", action.reload_config],
        ["o", "Show Root Keys", action.show_root],
      ]);
      sub.bind("t", "Theme", action.theme_next).stay();
    });

    m.mode("g", "go", [
      ["u", "u scratchpad", action.shell("yabai -m window --toggle web2")],
      ["y", "y scratchpad", action.shell("yabai -m window --toggle web")],
    ]);

    m.mode("w", "window", |sub, ctx| {
      sub.mode("2", "2 Columns", [
        ["h", "left", yabai_cell(2, 1, 0, 0)],
        ["l", "right", yabai_cell(2, 1, 1, 0)],
      ]);

      sub.mode("3", "3 Columns", |cols, ctx| {
        cols.bind([
          ["1", "Column 1", yabai_cell(3, 1, 0, 0)],
          ["2", "Column 2", yabai_cell(3, 1, 1, 0)],
          ["3", "Column 3", yabai_cell(3, 1, 2, 0)],
        ]);
        cols.bind([
          ["l", "left", yabai_cell_move(3, 1, 1, 0)],
          ["h", "right", yabai_cell_move(3, 1, -1, 0)],
        ]).stay();
      });

      sub.mode("g", "3x3 Grid", |grid, ctx| {
        grid.bind([
          ["l", "left", yabai_cell_move(3, 3, 1, 0)],
          ["h", "right", yabai_cell_move(3, 3, -1, 0)],
          ["k", "up", yabai_cell_move(3, 3, 0, -1)],
          ["j", "down", yabai_cell_move(3, 3, 0, 1)],
        ]).stay();
      });

      sub.mode("s", "move to space", [
        ["1", "1", action.shell("yabai -m window --space 1")],
        ["2", "2", action.shell("yabai -m window --space 2")],
        ["3", "3", action.shell("yabai -m window --space 3")],
        ["4", "4", action.shell("yabai -m window --space 4")],
        ["5", "5", action.shell("yabai -m window --space 5")],
        ["6", "6", action.shell("yabai -m window --space 6")],
        ["7", "7", action.shell("yabai -m window --space 7")],
        ["8", "8", action.shell("yabai -m window --space 8")],
        ["9", "9", action.shell("yabai -m window --space 9")],
      ]);

      sub.bind([
        ["f", "fullscreen", action.shell("yabai -m window --toggle zoom-fullscreen")],
        ["n", "native fullscreen", action.shell("yabai -m window --toggle native-fullscreen")],
        ["u", "set u scratchpad", action.shell("yabai -m window --scratchpad web2")],
        ["y", "set y scratchpad", action.shell("yabai -m window --scratchpad web")],
      ]);
    });

    m.bind("shift+cmd+space", "exit", action.exit).global().hidden();
  })
    .capture();

  // App-specific modal submenu based on focus context.
  if ctx.app.matches("WezTerm") {
    m.mode("shift+cmd+j", "wezterm", |m, ctx| {
      m.mode("r", "resize pane", |sub, ctx| {
        sub.bind([
          ["k", "up", action.relay("ctrl+cmd+shift+k")],
          ["j", "down", action.relay("ctrl+cmd+shift+j")],
          ["h", "left", action.relay("ctrl+cmd+shift+h")],
          ["l", "right", action.relay("ctrl+cmd+shift+l")],
        ]).stay();
      });

      m.mode("s", "split pane", [
        ["h", "left", action.relay("ctrl+shift+cmd+a")],
        ["l", "right", action.relay("ctrl+shift+cmd+s")],
        ["k", "up", action.relay("ctrl+shift+cmd+d")],
        ["j", "down", action.relay("ctrl+shift+cmd+f")],
      ]);

      m.bind("t", "toggle tabs", action.relay("ctrl+shift+cmd+t"));
    });
  } else if ctx.app.matches("Brave") {
    m.mode("shift+cmd+j", "brave", |m, ctx| {
      m.mode("b", "bookmarks", |sub, ctx| {
        sub.bind("b", "toggle bar", action.relay("shift+cmd+b"));
      });
    });
  } else if ctx.app.matches("Obsidian") {
    m.mode("shift+cmd+j", "obsidian", |m, ctx| {
      m.mode("c", "content", |sub, ctx| {
        sub.mode("i", "insert", [
          ["a", "attachment", action.relay("ctrl+shift+cmd+6")],
          ["c", "code", action.relay("ctrl+shift+cmd+7")],
          ["f", "footnote", action.relay("ctrl+shift+cmd+8")],
          ["l", "link", action.relay("ctrl+shift+cmd+9")],
        ]);

        sub.mode("t", "table", |inner, ctx| {
          inner.bind("n", "new", action.relay("ctrl+shift+cmd+y")).stay();
          inner.mode("i", "insert", |insert, ctx| {
            insert.bind([
              ["h", "column left", action.relay("ctrl+shift+cmd+u")],
              ["l", "column right", action.relay("ctrl+shift+cmd+i")],
              ["j", "column above", action.relay("ctrl+shift+cmd+o")],
              ["k", "column below", action.relay("ctrl+shift+cmd+p")],
            ]).stay();
          });
          inner.mode("d", "delete", |del, ctx| {
            del.bind([
              ["r", "row", action.relay("ctrl+shift+cmd+m")],
              ["c", "column", action.relay("ctrl+shift+cmd+n")],
            ]).stay();
          });
        });
      });

      m.mode("f", "file", [
        ["f", "show in finder", action.relay("ctrl+shift+cmd+4")],
        ["n", "rename", action.relay("ctrl+shift+cmd+5")],
      ]);

      m.mode("s", "split", [
        ["l", "right", action.relay("ctrl+shift+cmd+2")],
        ["j", "down", action.relay("ctrl+shift+cmd+1")],
      ]);

      m.mode("u", "utils", |sub, ctx| {
        sub.bind("s", "shell here", action.relay("ctrl+shift+cmd+s"));
      });

      m.mode("w", "window", [
        ["h", "sidebar left", action.relay("ctrl+shift+cmd+h")],
        ["l", "sidebar right", action.relay("ctrl+shift+cmd+l")],
        ["s", "settings", action.relay("ctrl+shift+cmd+3")],
      ]);
    });
  }
});
