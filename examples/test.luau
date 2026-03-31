themes:use("default")

local demo_items = {
    { label = "Calendar", sublabel = "demo://calendar", data = "calendar" },
    { label = "Calculator", sublabel = "demo://calculator", data = "calculator" },
    { label = "Callsheet", sublabel = "demo://callsheet", data = "callsheet" },
}

hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", action.pop, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "activate", function(root, inner)
        root:submenu("h", "Hotki", function(sub, subctx)
            sub:bind("c", "Clear Notifications", action.clear_notifications)
            sub:bind("d", "Details", action.show_details("toggle"))
            sub:bind("r", "Reload Config", action.reload_config)
            sub:bind("o", "Show Root Keys", action.show_root)
        end)

        root:submenu("c", "Key Capture", function(sub, subctx)
            sub:bind("a", "Hi", action.shell("echo hi", { ok_notify = "success", err_notify = "warn" }))
        end, { capture = true })

        root:submenu("s", "Shell", function(sub, subctx)
            sub:bind("e", "Simple echo", action.shell("echo hi", { ok_notify = "success", err_notify = "warn" }))
            sub:bind("s", "Sleep 5s", action.shell("sleep 5; echo slept 5s", { ok_notify = "success", err_notify = "warn" }))
            sub:bind("r", "Repeat Shell Default", action.shell("echo tick", { ok_notify = "success", err_notify = "warn" }), {
                ["repeat"] = { delay_ms = 250, interval_ms = 350 },
            })
            sub:bind("t", "Repeat Shell Custom", action.shell("echo tock", { ok_notify = "success", err_notify = "warn" }), {
                ["repeat"] = { delay_ms = 250, interval_ms = 350 },
            })
        end)

        root:submenu("r", "Relay", function(sub, subctx)
            sub:bind("a", "Send X", action.relay("shift+x"), { stay = true })
            sub:bind("b", "Repeat Relay Custom", action.relay("shift+x"), {
                ["repeat"] = { delay_ms = 200, interval_ms = 300 },
            })
        end)

        root:submenu("m", "Media", function(sub, subctx)
            sub:bind("p", "Play/Pause", action.shell("osascript -e 'tell application \"Spotify\" to playpause'"))
            sub:bind("n", "Next", action.shell("osascript -e 'tell application \"Spotify\" to next track'"))
            sub:bind("b", "Previous", action.shell("osascript -e 'tell application \"Spotify\" to previous track'"))
            sub:bind("1", "10%", action.set_volume(10), { stay = true })
            sub:bind("5", "50%", action.set_volume(50), { stay = true })
            sub:bind("m", "mute", action.mute("toggle"), { stay = true })
            sub:bind(
                "up", "Up 10", action.change_volume(10), { 
                    stay = true ,
                    ["repeat"] = { delay_ms = 250, interval_ms = 350 },
                }
            )
            sub:bind(
                "down", "Down 10", action.change_volume(-10), { 
                    stay = true,
                    ["repeat"] = { delay_ms = 250, interval_ms = 350 },
                }
            )
        end)

        root:submenu("t", "Theme tester", function(sub, subctx)
            sub:bind("h", "Theme Prev", action.theme_prev, { stay = true })
            sub:bind("l", "Theme Next", action.theme_next, { stay = true })
            sub:bind("1", "Dark Blue", action.theme_set("dark-blue"), { stay = true })
            sub:bind("2", "Charcoal", action.theme_set("charcoal"), { stay = true })
            sub:bind("3", "Default", action.theme_set("default"), { stay = true })
            sub:bind("s", "Success", action.shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            sub:bind("i", "Info", action.shell("echo info", { ok_notify = "info", err_notify = "warn" }), { stay = true })
            sub:bind("w", "Warning", action.shell("echo warn && false"), { stay = true })
            sub:bind("e", "Error", action.shell("echo error && false", { ok_notify = "ignore", err_notify = "error" }), { stay = true })
            sub:bind("a", "Run Application", action.selector({
                title = "Run Application",
                placeholder = "Search apps...",
                items = hotki.applications,
                on_select = function(actx: ActionContext, item: any, query: string)
                    actx:exec(action.open(item.data.path))
                end,
            }))
            sub:bind("p", "Selector Demo", action.selector({
                title = "Selector Demo",
                placeholder = "Search demo items...",
                items = demo_items,
                on_select = function(actx: ActionContext, item: any, query: string)
                    actx:notify("info", "Selected", item.label .. " (query: " .. query .. ")")
                end,
            }))
            sub:submenu("m", "Mini", function(mini, mini_ctx)
                mini:style({ hud = { mode = "mini" } })
                mini:bind("h", "Theme Prev", action.theme_prev, { stay = true })
                mini:bind("l", "Theme Next", action.theme_next, { stay = true })
            end)
        end)

        root:submenu("u", "Subtheme", function(sub, subctx)
            sub:style({ hud = { pos = "nw" } })
            sub:submenu("u", "Sub Subtheme", function(inner_sub, inner_ctx)
                inner_sub:style({ hud = { bg = "#ff0000" } })
                inner_sub:bind("h", "Theme Prev", action.theme_prev, { stay = true })
                inner_sub:bind("l", "Theme Next", action.theme_next, { stay = true })
            end)
        end)

        root:bind("shift+cmd+0", "exit", action.exit, { global = true, hidden = true })
    end)
end)
