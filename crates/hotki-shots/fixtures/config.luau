local demo_items: { SelectorItem<string> } = {
    { label = "Calendar", sublabel = "demo://calendar", data = "calendar" },
    { label = "Calculator", sublabel = "demo://calculator", data = "calculator" },
    { label = "Callsheet", sublabel = "demo://callsheet", data = "callsheet" },
}

hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", function(actx)
            actx:pop()
        end, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "Main", function(root, inner)
        root:submenu("h", "Hotki", function(sub, subctx)
            sub:bind("c", "Clear Notifications", function(actx)
                actx:clear_notifications()
            end)
            sub:bind("d", "Details", function(actx)
                actx:show_details("toggle")
            end)
            sub:bind("r", "Reload Config", function(actx)
                actx:reload_config()
            end)
            sub:bind("o", "Show Root Keys", function(actx)
                actx:show_root()
            end)
        end)

        root:submenu("n", "Notifications", function(sub, subctx)
            sub:bind("s", "Success", function(actx)
                actx:notify("success", "Success", "Operation completed")
            end, { stay = true })
            sub:bind("i", "Info", function(actx)
                actx:notify("info", "Info", "Background task started")
            end, { stay = true })
            sub:bind("w", "Warning", function(actx)
                actx:notify("warn", "Warning", "Check the current selection")
            end, { stay = true })
            sub:bind("e", "Error", function(actx)
                actx:notify("error", "Error", "Command failed")
            end, { stay = true })
            sub:bind("c", "Clear Notifications", function(actx)
                actx:clear_notifications()
            end, { stay = true })
            sub:bind("p", "Selector Demo", function(actx)
                actx:select({
                    title = "Selector Demo",
                    placeholder = "Search demo items...",
                    items = demo_items,
                    on_select = function(select_ctx: ActionContext, item: SelectorItem<string>, query: string)
                        select_ctx:notify("info", "Selected", item.label .. " (query: " .. query .. ")")
                    end,
                })
            end)
        end)

        root:submenu("m", "Media", function(sub, subctx)
            sub:bind("1", "10%", function(actx)
                actx:set_volume(10)
            end, { stay = true })
            sub:bind("5", "50%", function(actx)
                actx:set_volume(50)
            end, { stay = true })
            sub:bind("m", "Mute", function(actx)
                actx:mute("toggle")
            end, { stay = true })
        end)

        root:bind("shift+cmd+0", "Exit", function(actx)
            actx:exit()
        end, { global = true, hidden = true })
    end)
end)
