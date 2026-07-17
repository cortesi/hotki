local a = hotki.actions

local demo_items: { SelectorItem<string> } = {
    { label = "Calendar", sublabel = "demo://calendar", data = "calendar" },
    { label = "Calculator", sublabel = "demo://calculator", data = "calculator" },
    { label = "Callsheet", sublabel = "demo://callsheet", data = "callsheet" },
}

return function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", a.pop, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "activate", function(root)
        root:submenu("h", "Hotki", function(sub)
            sub:bind("c", "Clear Notifications", a.clear_notifications)
            sub:bind("d", "Hotki", a.show_main_window("toggle"))
            sub:bind("r", "Reload Config", a.reload_config)
            sub:bind("o", "Show Root Keys", a.show_root)
        end)

        root:submenu("c", "Key Capture", function(sub)
            sub:bind("a", "Hi", a.shell("echo hi", { ok_notify = "success", err_notify = "warn" }))
        end, { capture = true })

        root:submenu("s", "Shell", function(sub)
            sub:bind("e", "Simple echo", a.shell("echo hi", { ok_notify = "success", err_notify = "warn" }))
            sub:bind("s", "Sleep 5s", a.shell("sleep 5; echo slept 5s", { ok_notify = "success", err_notify = "warn" }))
            sub:bind("r", "Repeat Shell Default", a.hold(a.shell("echo tick", { ok_notify = "success", err_notify = "warn" }), {
                delay_ms = 250,
                interval_ms = 350,
            }))
            sub:bind("t", "Repeat Shell Custom", a.hold(a.shell("echo tock", { ok_notify = "success", err_notify = "warn" }), {
                delay_ms = 250,
                interval_ms = 350,
            }))
        end)

        root:submenu("r", "Relay", function(sub)
            sub:bind("a", "Send X", a.relay("shift+x"), { stay = true })
            sub:bind("b", "Repeat Relay Custom", a.hold(a.relay("shift+x"), {
                delay_ms = 200,
                interval_ms = 300,
            }))
        end)

        root:submenu("m", "Media", function(sub)
            sub:bind("p", "Play/Pause", a.shell("osascript -e 'tell application \"Spotify\" to playpause'"))
            sub:bind("n", "Next", a.shell("osascript -e 'tell application \"Spotify\" to next track'"))
            sub:bind("b", "Previous", a.shell("osascript -e 'tell application \"Spotify\" to previous track'"))
            sub:bind("1", "10%", a.set_volume(10), { stay = true })
            sub:bind("5", "50%", a.set_volume(50), { stay = true })
            sub:bind("m", "mute", a.mute("toggle"), { stay = true })
            sub:bind(
                "up", "Up 10", a.hold(a.change_volume(10), {
                    delay_ms = 250,
                    interval_ms = 350,
                }), { stay = true }
            )
            sub:bind(
                "down", "Down 10", a.hold(a.change_volume(-10), {
                    delay_ms = 250,
                    interval_ms = 350,
                }), { stay = true }
            )
        end)

        root:submenu("n", "Notifications", function(sub)
            sub:bind("s", "Success", a.shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            sub:bind("i", "Info", a.shell("echo info", { ok_notify = "info", err_notify = "warn" }), { stay = true })
            sub:bind("w", "Warning", a.shell("echo warn && false"), { stay = true })
            sub:bind("e", "Error", a.shell("echo error && false", { ok_notify = "ignore", err_notify = "error" }), { stay = true })
            sub:bind("a", "Run Application", a.select({
                    title = "Run Application",
                    placeholder = "Search apps...",
                    items = hotki.applications,
                    on_select = function(select_ctx, item)
                        select_ctx:open(item.data.path)
                    end,
                }))
            sub:bind("p", "Selector Demo", a.select({
                    title = "Selector Demo",
                    placeholder = "Search demo items...",
                    items = demo_items,
                    on_select = function(select_ctx, item, query)
                        select_ctx:notify("info", "Selected", item.label .. " (query: " .. query .. ")")
                    end,
                }))
            sub:submenu("m", "Mini", function(mini)
                mini:bind("s", "Success", a.shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            end)
        end)

        root:bind("shift+cmd+0", "exit", a.exit, { global = true, hidden = true })
    end)
end
