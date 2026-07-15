local a = hotki.actions

return function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Exit", a.exit, { global = true, hidden = true })
    end

    menu:bind("shift+cmd+p", "Run Application", a.launch_application())

    menu:submenu("shift+cmd+0", "Actions", function(actions)
        local stay = actions:with({ stay = true })
        actions:bind("p", "Pop", a.pop)
        actions:bind("e", "Exit", a.exit)
        actions:bind("r", "Show Root", a.show_root)
        actions:bind("h", "Hide HUD", a.hide_hud)
        actions:bind("c", "Clear Notifications", a.clear_notifications)
        actions:bind("shift+r", "Reload Config", a.reload_config)
        actions:bind("n", "Notify", a.notify("success", "Hotki", "Action complete"))
        actions:bind("s", "Shell", a.shell("echo hello", {
            ok_notify = "success",
            err_notify = "error",
        }))
        actions:bind("i", "Direct process", a.exec({
            program = "/usr/bin/printf",
            args = { "direct execution" },
            ok_notify = "info",
        }))
        actions:bind("o", "Open", a.open("https://github.com/cortesi/hotki"))
        actions:bind("k", "Relay", a.relay("cmd+c"))
        actions:bind("y", "YouTube Music Play/Pause", a.relay_to_app("YouTube Music")("space"))
        stay:bind("d", "Details", a.show_details("toggle"))
        stay:bind("v", "Volume 50%", a.set_volume(50))
        stay:bind("=", "Volume Up", a.change_volume(5))
        stay:bind("-", "Volume Down", a.change_volume(-5))
        stay:bind("m", "Mute", a.mute("toggle"))
        stay:bind("[", "Unmute", a.mute("off"))
        actions:bind("t", "Stay", a.stay)
        stay:bind("u", "Hold Volume Up", a.hold(a.change_volume(5)))
        actions:bind("x", "Composite Action", function(action_ctx)
            action_ctx:shell("echo composite")
            action_ctx:notify("info", "Composite", "Queued two ordered effects")
            action_ctx:stay()
        end)
        actions:bind("g", "Push Mode", a.push(function(child)
            child:bind("esc", "Back", a.pop, { global = true, hidden = true })
            child:bind("s", "Nested Shell", a.shell("echo nested"))
        end, "Pushed"))
    end, { capture = true })

    if ctx:app_matches("Finder") then
        menu:bind("shift+cmd+f", "Finder Applications", a.open("/Applications"), {
            global = true,
            hidden = true,
        })
    end
end
