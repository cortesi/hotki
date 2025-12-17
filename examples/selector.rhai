// Selector example: Spotlight-style "run application" launcher.

theme("default");

hotki.mode(|m, ctx| {
  m.bind(
    "shift+cmd+0",
    "Run Application",
    action.selector(#{
      title: "Run Application",
      placeholder: "Search apps...",
      items: || get_applications(),
      on_select: |ctx, item, query| {
        ctx.exec(action.shell("open '" + item.data + "'"))
      },
    }),
  );
});

