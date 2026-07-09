local demo_items: { SelectorItem<string> } = {
    { label = "Calendar", sublabel = "demo://calendar", data = "calendar" },
    { label = "Calculator", sublabel = "demo://calculator", data = "calculator" },
    { label = "Callsheet", sublabel = "demo://callsheet", data = "callsheet" },
}

hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", action.pop, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "Main", function(root, inner)
        root:submenu("h", "Hotki", function(sub, subctx)
            sub:bind("c", "Clear Notifications", action.clear_notifications)
            sub:bind("d", "Details", action.show_details("toggle"))
            sub:bind("r", "Reload Config", action.reload_config)
            sub:bind("o", "Show Root Keys", action.show_root)
        end)

        root:submenu("n", "Notifications", function(sub, subctx)
            sub:bind("s", "Success", action.run(function(actx)
                actx:notify("success", "Success", "Operation completed")
            end), { stay = true })
            sub:bind("i", "Info", action.run(function(actx)
                actx:notify("info", "Info", "Background task started")
            end), { stay = true })
            sub:bind("w", "Warning", action.run(function(actx)
                actx:notify("warn", "Warning", "Check the current selection")
            end), { stay = true })
            sub:bind("e", "Error", action.run(function(actx)
                actx:notify("error", "Error", "Command failed")
            end), { stay = true })
            sub:bind("c", "Clear Notifications", action.clear_notifications, { stay = true })
            sub:bind("p", "Selector Demo", action.selector({
                title = "Selector Demo",
                placeholder = "Search demo items...",
                items = demo_items,
                on_select = function(actx: ActionContext, item: SelectorItem<string>, query: string)
                    actx:notify("info", "Selected", item.label .. " (query: " .. query .. ")")
                end,
            }))
        end)

        root:submenu("m", "Media", function(sub, subctx)
            sub:bind("1", "10%", action.set_volume(10), { stay = true })
            sub:bind("5", "50%", action.set_volume(50), { stay = true })
            sub:bind("m", "Mute", action.mute("toggle"), { stay = true })
        end)

        root:bind("shift+cmd+0", "Exit", action.exit, { global = true, hidden = true })
    end)
end)
