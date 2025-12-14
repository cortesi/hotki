// A rough faximile of cortesi's yabai + wezterm + spotify config
//
// This config assumes:
// - `yabai` is installed and running
// - `spotify` is available on PATH (or adjust commands)
// - `/opt/homebrew/bin/wezterm` exists (or adjust)

base_theme("default");

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

global.bind("cmd+1", "1", action.shell("yabai -m space --focus 1"));
global.bind("cmd+2", "2", action.shell("yabai -m space --focus 2"));
global.bind("cmd+3", "3", action.shell("yabai -m space --focus 3"));
global.bind("cmd+4", "4", action.shell("yabai -m space --focus 4"));
global.bind("cmd+5", "5", action.shell("yabai -m space --focus 5"));
global.bind("cmd+6", "6", action.shell("yabai -m space --focus 6"));
global.bind("cmd+7", "7", action.shell("yabai -m space --focus 7"));
global.bind("cmd+8", "8", action.shell("yabai -m space --focus 8"));
global.bind("cmd+9", "9", action.shell("yabai -m space --focus 9"));
global.bind("cmd+0", "0", action.shell("yabai -m space --focus 10"));

global.bind(
  "cmd+return",
  "0",
  action.shell("/opt/homebrew/bin/wezterm cli spawn --new-window")
);

global.bind("shift+cmd+y", "y", action.shell("yabai -m window --toggle web"));
global.bind("shift+cmd+u", "u", action.shell("yabai -m window --toggle web2"));

global
  .mode("shift+cmd+m", "music", |m| {
    m.bind("k", "vol up", action.change_volume(5)).no_exit().repeat();
    m.bind("j", "vol down", action.change_volume(-5)).no_exit().repeat();
    m.bind("l", "next >>", action.shell("spotify next"));
    m.bind("h", "<< prev", action.shell("spotify prev"));
    m.bind("p", "play/pause", action.shell("spotify pause"));
    m.bind("m", "mute", action.mute(toggle));
    m.bind("shift+cmd+m", "exit", action.exit).global().hidden();
  })
  .capture();

global
  .mode("shift+cmd+space", "activate", |m| {
    m.mode("h", "hotki", |sub| {
      sub.bind("c", "Clear Notifications", action.clear_notifications);
      sub.bind("d", "Details", action.show_details(toggle));
      sub.bind("r", "Reload Config", action.reload_config);
      sub.bind("o", "Show Root Keys", action.show_hud_root);
      sub.bind("t", "Theme", action.theme_next).no_exit();
      sub.bind("u", "User UI", action.user_style(toggle)).no_exit();
    });

    m.mode("g", "go", |sub| {
      sub.bind("u", "u scratchpad", action.shell("yabai -m window --toggle web2"));
      sub.bind("y", "y scratchpad", action.shell("yabai -m window --toggle web"));
    });

    m.mode("w", "window", |sub| {
      sub.mode("2", "2 Columns", |cols| {
        cols.bind("h", "left", yabai_cell(2, 1, 0, 0));
        cols.bind("l", "right", yabai_cell(2, 1, 1, 0));
      });

      sub.mode("3", "3 Columns", |cols| {
        cols.bind("1", "Column 1", yabai_cell(3, 1, 0, 0));
        cols.bind("2", "Column 2", yabai_cell(3, 1, 1, 0));
        cols.bind("3", "Column 3", yabai_cell(3, 1, 2, 0));
        cols
          .bind("l", "left", yabai_cell_move(3, 1, 1, 0))
          .no_exit();
        cols
          .bind("h", "right", yabai_cell_move(3, 1, -1, 0))
          .no_exit();
      });

      sub.mode("g", "3x3 Grid", |grid| {
        grid
          .bind("l", "left", yabai_cell_move(3, 3, 1, 0))
          .no_exit();
        grid
          .bind("h", "right", yabai_cell_move(3, 3, -1, 0))
          .no_exit();
        grid
          .bind("k", "up", yabai_cell_move(3, 3, 0, -1))
          .no_exit();
        grid
          .bind("j", "down", yabai_cell_move(3, 3, 0, 1))
          .no_exit();
      });

      sub.mode("s", "move to space", |spaces| {
        spaces.bind("1", "1", action.shell("yabai -m window --space 1"));
        spaces.bind("2", "2", action.shell("yabai -m window --space 2"));
        spaces.bind("3", "3", action.shell("yabai -m window --space 3"));
        spaces.bind("4", "4", action.shell("yabai -m window --space 4"));
        spaces.bind("5", "5", action.shell("yabai -m window --space 5"));
        spaces.bind("6", "6", action.shell("yabai -m window --space 6"));
        spaces.bind("7", "7", action.shell("yabai -m window --space 7"));
        spaces.bind("8", "8", action.shell("yabai -m window --space 8"));
        spaces.bind("9", "9", action.shell("yabai -m window --space 9"));
      });

      sub.bind("f", "fullscreen", action.shell("yabai -m window --toggle zoom-fullscreen"));
      sub.bind(
        "n",
        "native fullscreen",
        action.shell("yabai -m window --toggle native-fullscreen")
      );
      sub.bind(
        "u",
        "set u scratchpad",
        action.shell("yabai -m window --scratchpad web2")
      );
      sub.bind("y", "set y scratchpad", action.shell("yabai -m window --scratchpad web"));
    });

    m.bind("shift+cmd+space", "exit", action.exit).global().hidden();
  })
  .capture();

global
  .mode("shift+cmd+j", "wezterm", |m| {
    m.mode("r", "resize pane", |sub| {
      sub.bind("k", "up", action.relay("ctrl+cmd+shift+k")).no_exit();
      sub.bind("j", "down", action.relay("ctrl+cmd+shift+j")).no_exit();
      sub.bind("h", "left", action.relay("ctrl+cmd+shift+h")).no_exit();
      sub.bind("l", "right", action.relay("ctrl+cmd+shift+l")).no_exit();
    });

    m.mode("s", "split pane", |sub| {
      sub.bind("h", "left", action.relay("ctrl+shift+cmd+a"));
      sub.bind("l", "right", action.relay("ctrl+shift+cmd+s"));
      sub.bind("k", "up", action.relay("ctrl+shift+cmd+d"));
      sub.bind("j", "down", action.relay("ctrl+shift+cmd+f"));
    });

    m.bind("t", "toggle tabs", action.relay("ctrl+shift+cmd+t"));
  })
  .match_app("WezTerm");

global
  .mode("shift+cmd+j", "brave", |m| {
    m.mode("b", "bookmarks", |sub| {
      sub.bind("b", "toggle bar", action.relay("shift+cmd+b"));
    });
  })
  .match_app("Brave");

global
  .mode("shift+cmd+j", "obsidian", |m| {
    m.mode("c", "content", |sub| {
      sub.mode("i", "insert", |inner| {
        inner.bind("a", "attachment", action.relay("ctrl+shift+cmd+6"));
        inner.bind("c", "code", action.relay("ctrl+shift+cmd+7"));
        inner.bind("f", "footnote", action.relay("ctrl+shift+cmd+8"));
        inner.bind("l", "link", action.relay("ctrl+shift+cmd+9"));
      });

      sub.mode("t", "table", |inner| {
        inner.bind("n", "new", action.relay("ctrl+shift+cmd+y")).no_exit();
        inner.mode("i", "insert", |insert| {
          insert.bind("h", "column left", action.relay("ctrl+shift+cmd+u")).no_exit();
          insert.bind("l", "column right", action.relay("ctrl+shift+cmd+i")).no_exit();
          insert.bind("j", "column above", action.relay("ctrl+shift+cmd+o")).no_exit();
          insert.bind("k", "column below", action.relay("ctrl+shift+cmd+p")).no_exit();
        });
        inner.mode("d", "delete", |del| {
          del.bind("r", "row", action.relay("ctrl+shift+cmd+m")).no_exit();
          del.bind("c", "column", action.relay("ctrl+shift+cmd+n")).no_exit();
        });
      });
    });

    m.mode("f", "file", |sub| {
      sub.bind("f", "show in finder", action.relay("ctrl+shift+cmd+4"));
      sub.bind("n", "rename", action.relay("ctrl+shift+cmd+5"));
    });

    m.mode("s", "split", |sub| {
      sub.bind("l", "right", action.relay("ctrl+shift+cmd+2"));
      sub.bind("j", "down", action.relay("ctrl+shift+cmd+1"));
    });

    m.mode("u", "utils", |sub| {
      sub.bind("s", "shell here", action.relay("ctrl+shift+cmd+s"));
    });

    m.mode("w", "window", |sub| {
      sub.bind("h", "sidebar left", action.relay("ctrl+shift+cmd+h"));
      sub.bind("l", "sidebar right", action.relay("ctrl+shift+cmd+l"));
      sub.bind("s", "settings", action.relay("ctrl+shift+cmd+3"));
    });
  })
  .match_app("Obsidian");

global.bind("esc", "Exit", action.exit).global().hidden().hud_only();
global.bind("left", "Pop", action.pop).global().hidden().hud_only();
