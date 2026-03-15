themes:use("default")

hotki.root(function(menu, ctx)
    menu:style({
        hud = {
            tag_submenu = ">>",
            pos = "ne",
            offset = { x = -20, y = -40 },
            font_size = 16,
            key_font_size = 14,
            tag_font_size = 12,
            title_font_weight = "medium",
            key_font_weight = "semibold",
            tag_font_weight = "bold",
            mod_font_weight = "extrabold",
            title_fg = "#ffffff",
            bg = "#1a1a1a",
            key_fg = "white",
            key_bg = "#2a2a2a",
            mod_fg = "#00ff00",
            mod_bg = "#3a3a3a",
            tag_fg = "yellow",
            opacity = 0.95,
            radius = 12,
            key_radius = 4,
            key_pad_x = 8,
            key_pad_y = 4,
        },
        notify = {
            width = 400,
            pos = "right",
            opacity = 0.9,
            timeout = 3.5,
            buffer = 4,
            radius = 8,
            info = { bg = "#1e1e1e", title_fg = "#88ccff", body_fg = "#cccccc", title_font_size = 14, title_font_weight = "bold", body_font_size = 12, body_font_weight = "regular", icon = "\u{ea74}" },
            warn = { bg = "#332200", title_fg = "#ffaa00", body_fg = "#ffcc66", title_font_size = 14, title_font_weight = "semibold", body_font_size = 12, body_font_weight = "light", icon = "\u{ea6c}" },
            error = { bg = "#330000", title_fg = "#ff4444", body_fg = "#ff8888", title_font_size = 15, title_font_weight = "extrabold", body_font_size = 12, body_font_weight = "medium", icon = "\u{ea87}" },
            success = { bg = "#003300", title_fg = "#00ff00", body_fg = "#88ff88", title_font_size = 14, title_font_weight = "medium", body_font_size = 12, body_font_weight = "regular", icon = "\u{f4b2}" },
        },
    })

    if ctx.hud then
        menu:bind("esc", "Back", action.pop, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "Main", function(root, inner)
        root:bind("shift+cmd+0", "Exit", action.exit, { global = true, hidden = true })
        root:bind("s", "Shell Command", action.shell("echo 'Hello'"))
        root:bind("shift+S", "Shell with Notifications", action.shell("echo 'Success'", { ok_notify = "success", err_notify = "error" }))
        root:bind("r", "Relay Keystroke", action.relay("cmd+c"))
        root:bind("c", "Clear Notifications", action.clear_notifications)
        root:bind("d", "Toggle Details", action.show_details("toggle"), { stay = true })
        root:bind("shift+R", "Reload Config", action.reload_config)
        root:bind("h", "Show Root HUD", action.show_root)
        root:bind("shift+H", "Hide HUD", action.hide_hud)
        root:bind("1", "Dark Blue Theme", action.theme_set("dark-blue"), { stay = true })
        root:bind("2", "Charcoal Theme", action.theme_set("charcoal"), { stay = true })
        root:bind("3", "Default Theme", action.theme_set("default"), { stay = true })
        root:bind("v", "Set Volume 50%", action.set_volume(50), { stay = true })
        root:bind("shift+V", "Change Volume +10", action.change_volume(10), { stay = true })
        root:bind("m", "Mute", action.mute("on"), { stay = true })
        root:bind("u", "Unmute", action.mute("off"), { stay = true })
        root:bind("shift+M", "Toggle Mute", action.mute("toggle"), { stay = true })
        root:bind("x", "Complex Action", action.run(function(actx)
            actx:exec(action.shell("echo hello"))
            actx:notify("success", "Done", "Queued a shell command + notification")
            actx:stay()
        end))
        root:bind("g", "Global Key", action.shell("echo 'Global'"), { global = true })
        root:bind("shift+X", "Hidden Key", action.shell("echo 'Hidden'"), { hidden = true })
        root:bind("a", "Repeat Shell", action.shell("echo tick"), {
            ["repeat"] = { delay_ms = 250, interval_ms = 350 },
        })
        root:bind("b", "Repeat Relay", action.relay("cmd+c"), {
            ["repeat"] = { delay_ms = 250, interval_ms = 350 },
        })
        root:submenu("t", "Theme Tester", function(sub, subctx)
            sub:bind("n", "Next Theme", action.theme_next, { stay = true })
            sub:bind("p", "Previous Theme", action.theme_prev, { stay = true })
        end)
    end, { capture = true })
end)
