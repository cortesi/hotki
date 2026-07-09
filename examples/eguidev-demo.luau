hotki.root(function(menu, ctx)
  menu:submenu("shift+cmd+0", "activate", function(activate, inner)
    activate:submenu("d", "Demo", function(sub, subctx)
      sub:bind("n", "Notify", function(actx)
        actx:shell("echo notify", {
          ok_notify = "info",
          err_notify = "warn",
        })
      end, { stay = true })
      sub:bind("d", "Details", function(actx)
        actx:show_details("on")
      end, { stay = true })
      sub:bind("s", "Selector", function(actx)
        actx:select({
          title = "Pick Demo",
          placeholder = "Search...",
          items = { "Alpha", "Beta" },
          on_select = function(select_ctx, item, query)
            select_ctx:notify("info", "Selector", item.label .. ":" .. query)
          end,
          on_cancel = function(select_ctx)
            select_ctx:notify("warn", "Selector", "cancel")
          end,
        })
      end, { stay = true })
      sub:submenu("m", "Mini", function(mini, mini_ctx)
        mini:bind("n", "Notify", function(actx)
          actx:shell("echo mini", { ok_notify = "info" })
        end, { stay = true })
        mini:bind("esc", "Exit", function(actx)
          actx:exit()
        end, { hidden = true })
      end)
      sub:bind("esc", "Exit", function(actx)
        actx:exit()
      end, { hidden = true })
    end)
  end)
end)
