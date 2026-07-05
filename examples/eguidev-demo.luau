themes:use("default")

hotki.root(function(menu, ctx)
  menu:style({
    hud = {
      mode = "hud",
      pos = "se",
    },
    notify = {
      pos = "left",
    },
  })

  menu:submenu("shift+cmd+0", "activate", function(activate, inner)
    activate:submenu("t", "Theme tester", function(sub, subctx)
      sub:bind("h", "Theme Prev", action.theme_prev, { stay = true })
      sub:bind("l", "Theme Next", action.theme_next, { stay = true })
      sub:bind("n", "Notify", action.shell("echo notify", {
        ok_notify = "info",
        err_notify = "warn",
      }), { stay = true })
      sub:bind("d", "Details", action.show_details("on"), { stay = true })
      sub:bind("s", "Selector", action.selector({
        title = "Pick Demo",
        placeholder = "Search...",
        items = { "Alpha", "Beta" },
        on_select = function(actx, item, query)
          actx:notify("info", "Selector", item.label .. ":" .. query)
        end,
        on_cancel = function(actx)
          actx:notify("warn", "Selector", "cancel")
        end,
      }), { stay = true })
      sub:submenu("m", "Mini", function(mini, mini_ctx)
        mini:style({ hud = { mode = "mini" } })
        mini:bind("h", "Theme Prev", action.theme_prev, { stay = true })
        mini:bind("l", "Theme Next", action.theme_next, { stay = true })
        mini:bind("esc", "Exit", action.exit, { hidden = true })
      end)
      sub:bind("esc", "Exit", action.exit, { hidden = true })
    end)
  end)
end)
