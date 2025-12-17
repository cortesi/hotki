// Selector example: custom item list with notifications.

theme("default");

let items = [
  "Alpha",
  "Beta",
  "Gamma",
  #{ label: "Delta", sublabel: "Custom sublabel", data: 4 },
  selector_item("Epsilon", "ep"),
  selector_item("Zeta", "ze"),
];

hotki.mode(|m, ctx| {
  m.bind(
    "shift+cmd+0",
    "Pick Item",
    action.selector(#{
      title: "Pick Item",
      placeholder: "Type to filter...",
      items: items,
      on_select: |ctx, item, query| {
        ctx.notify(info, "Selected", `${item.label} (query: ${query})`);
      },
      on_cancel: |ctx| {
        ctx.notify(info, "Selector", "Cancelled");
      },
    }),
  );
});

