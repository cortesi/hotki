// Comprehensive config used by hotki-shots and manual testing.

base_theme("default");

global.mode("shift+cmd+0", "activate", |m| {
  m.mode("h", "Hotki", |sub| {
    sub.bind("c", "Clear Notifications", action.clear_notifications);
    sub.bind("d", "Details", action.show_details(toggle));
    sub.bind("r", "Reload Config", action.reload_config);
    sub.bind("o", "Show Root Keys", action.show_hud_root);
  });

  m.mode("c", "Key Capture", |sub| {
    sub.bind("a", "Hi", action.shell("echo hi").notify(success, warn));
  })
    .capture();

  m.mode("s", "Shell", |sub| {
    sub.bind("e", "Simple echo", action.shell("echo hi").notify(success, warn));
    sub.bind("s", "Sleep 5s", action.shell("sleep 5; echo slept 5s").notify(success, warn));

    // Shell repeat examples (repeat settings live in binding attrs).
    sub.bind("r", "Repeat Shell Default", action.shell("echo tick").notify(success, warn))
      .no_exit()
      .repeat();
    sub.bind("t", "Repeat Shell Custom", action.shell("echo tock").notify(success, warn))
      .no_exit()
      .repeat_ms(250, 350);
  });

  m.mode("r", "Relay", |sub| {
    sub.bind("a", "Send X", action.relay("shift+x")).no_exit();

    // Relay repeat examples (attrs apply to relay too).
    sub.bind("b", "Repeat Relay Custom", action.relay("shift+x"))
      .no_exit()
      .repeat_ms(200, 300);
  });

  m.mode("m", "Media", |sub| {
    // For media playback, use shell commands specific to your apps.
    // Examples for Spotify:
    sub.bind(
      "space",
      "Play/Pause",
      action.shell("osascript -e 'tell application \"Spotify\" to playpause'")
    );
    sub.bind("n", "Next", action.shell("osascript -e 'tell application \"Spotify\" to next track'"));
    sub.bind(
      "b",
      "Previous",
      action.shell("osascript -e 'tell application \"Spotify\" to previous track'")
    );

    sub.bind("1", "10%", action.set_volume(10)).no_exit();
    sub.bind("5", "50%", action.set_volume(50)).no_exit();
    sub.bind("m", "mute", action.mute(toggle)).no_exit();
    sub.bind("up", "Up 10", action.change_volume(10)).no_exit();
    sub.bind("down", "Down 10", action.change_volume(-10)).no_exit();
  });

  m.mode("t", "Theme tester", |sub| {
    sub.bind("h", "Theme Prev", action.theme_prev).no_exit();
    sub.bind("l", "Theme Next", action.theme_next).no_exit();

    sub
      .bind(
        "s",
        "Success",
        action.shell("echo this is a success window").notify(success, warn)
      )
      .no_exit();
    sub
      .bind("i", "Info", action.shell("echo this is an info window").notify(info, warn))
      .no_exit();
    sub.bind("w", "Warning", action.shell("echo this is a warning window && false")).no_exit();
    sub
      .bind(
        "e",
        "Error",
        action.shell("echo this is an error window && false").notify(ignore, error)
      )
      .no_exit();
    sub.bind("u", "User Style", action.user_style(toggle)).no_exit();

    sub
      .mode("m", "Mini", |mini| {
        mini.bind("h", "Theme Prev", action.theme_prev).no_exit();
        mini.bind("l", "Theme Next", action.theme_next).no_exit();
      })
      .style(#{ hud: #{ mode: hud_mini } });

    sub.mode("shift+l", "Long Submode", |long| {
      long.bind("a", "Inner Action A", action.shell("echo here"));
      long.bind("b", "Inner Action B", action.shell("echo here"));
      long.bind("c", "Inner Action C", action.shell("echo here"));
      long.bind("d", "Inner Action D", action.shell("echo here"));
      long.bind("e", "Inner Action E", action.shell("echo here"));
      long.bind("f", "Inner Action F", action.shell("echo here"));
      long.bind("g", "Inner Action G", action.shell("echo here"));
      long.bind("h", "Inner Action H", action.shell("echo here"));
      long.mode("ctrl+cmd+j", "Inner Mode with a very long title", |inner| {
        inner.bind("i", "Inner Mode", action.shell("echo Inner Mode"));
        inner.bind("r", "Return to Main Menu", action.pop);
      });
      long.bind(
        "ctrl+cmd+i",
        "Inner Action with a very long title",
        action.shell("echo here")
      );
    });

    sub
      .bind(
        "shift+s",
        "Long Success",
        action.shell(
          `
                    awk -v F=%c '
                    BEGIN{
                      srand()
                      for(i=10;i--;){
                        for(j=int(rand()*10)+1;j--;){
                          for(k=int(rand()*9)+2;k--;) printf F, 97+int(rand()*26)
                          printf j?OFS:ORS
                        }
                      }
                      exit
                    }'
                `
        )
          .notify(success, warn)
      )
      .no_exit();
    sub
      .bind(
        "shift+i",
        "Long Info",
        action.shell(
          `
                    awk -v F=%c '
                    BEGIN{
                      srand()
                      for(i=10;i--;){
                        for(j=int(rand()*10)+1;j--;){
                          for(k=int(rand()*9)+2;k--;) printf F, 97+int(rand()*26)
                          printf j?OFS:ORS
                        }
                      }
                      exit
                    }'
                `
        )
          .notify(info, warn)
      )
      .no_exit();
    sub
      .bind(
        "shift+w",
        "Long Warning",
        action.shell(
          `
                    awk -v F=%c '
                    BEGIN{
                      srand()
                      for(i=10;i--;){
                        for(j=int(rand()*10)+1;j--;){
                          for(k=int(rand()*9)+2;k--;) printf F, 97+int(rand()*26)
                          printf j?OFS:ORS
                        }
                      }
                      exit
                    }'  && false
                `
        )
      )
      .no_exit();
    sub
      .bind(
        "shift+e",
        "Long Error",
        action.shell(
          `
                    awk -v F=%c '
                    BEGIN{
                      srand()
                      for(i=10;i--;){
                        for(j=int(rand()*10)+1;j--;){
                          for(k=int(rand()*9)+2;k--;) printf F, 97+int(rand()*26)
                          printf j?OFS:ORS
                        }
                      }
                      exit
                    }'  && false
                `
        )
          .notify(ignore, error)
      )
      .no_exit();
  });

  m.mode("u", "Subtheme", |sub| {
    sub
      .mode("u", "Sub Subtheme", |sub_sub| {
        sub_sub.bind("h", "Theme Prev", action.theme_prev).no_exit();
        sub_sub.bind("l", "Theme Next", action.theme_next).no_exit();
      })
      .style(#{ hud: #{ bg: "#ff0000" } });
  })
    .style(#{ hud: #{ pos: nw } });

  m.bind("shift+cmd+0", "exit", action.exit).global().hidden();
});

global.mode("shift+cmd+9", "activate", |m| {
  m.bind("a", "Inner Action A", action.shell("echo here"));
  m.bind("b", "Inner Action B", action.shell("echo here"));
  m.bind("c", "Inner Action C", action.shell("echo here"));
  m.bind("d", "Inner Action D", action.shell("echo here"));
  m.bind("e", "Inner Action E", action.shell("echo here"));
  m.bind("f", "Inner Action F", action.shell("echo here"));
  m.bind("g", "Inner Action G", action.shell("echo here"));
  m.bind("h", "Inner Action H", action.shell("echo here"));
  m.bind("ctrl+cmd+i", "Inner Action I", action.shell("echo here"));

  m.bind("shift+cmd+9", "exit", action.exit).global().hidden();
});

global.bind("esc", "Back", action.pop).global().hidden().hud_only();
