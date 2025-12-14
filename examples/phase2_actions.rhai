// Phase 2 example: Rhai-native actions (closures/functions) that compute Actions at runtime.

base_theme("default");

fn spotify(cmd) {
  action.shell("spotify " + cmd).notify(ignore, warn)
}

global.mode("shift+cmd+m", "Music", |m| {
  // Closure action: compute an Action at runtime.
  m.bind("p", "Play/Pause", || spotify("pause"));

  // Context-aware closure action: branch based on focused app.
  m.bind("o", "Open Logs", |ctx| {
    if ctx.app.contains("Obsidian") {
      action.shell("open -a Obsidian ~/logs/obsidian.log")
    } else {
      action.shell("open ~/logs/system.log")
    }
  });

  // "Macro" action: return a sequence of Actions (executed in-order).
  m.bind("s", "Save + Beep", || [
    action.relay("cmd+s"),
    action.shell("afplay /System/Library/Sounds/Pop.aiff").silent(),
  ]);

  m.bind("esc", "Back", action.pop).global().hidden();
}).capture();
