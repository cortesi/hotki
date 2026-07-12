local a = hotki.actions

local GLOBAL = { global = true, hidden = true }
local STAY = { stay = true }

return function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Exit", a.exit, GLOBAL)
    end

    menu:bind("shift+cmd+p", "Run Application", a.select({
        title = "Run Application",
        placeholder = "Search apps...",
        items = hotki.applications,
        on_select = function(select_ctx, item)
            select_ctx:open(item.data.path)
        end,
    }))

    menu:submenu("shift+cmd+0", "Actions", function(actions)
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
        actions:bind("o", "Open", a.open("https://github.com/cortesi/hotki"))
        actions:bind("k", "Relay", a.relay("cmd+c"))
        actions:bind("d", "Details", a.show_details("toggle"), STAY)
        actions:bind("v", "Volume 50%", a.set_volume(50), STAY)
        actions:bind("=", "Volume Up", a.change_volume(5), STAY)
        actions:bind("-", "Volume Down", a.change_volume(-5), STAY)
        actions:bind("m", "Mute", a.mute("toggle"), STAY)
        actions:bind("t", "Stay", a.stay)
        actions:bind("u", "Hold Volume Up", a.hold(a.change_volume(5), {
            delay_ms = 250,
            interval_ms = 100,
        }), STAY)
        actions:bind("x", "Composite Action", function(action_ctx)
            action_ctx:shell("echo composite")
            action_ctx:notify("info", "Composite", "Queued two ordered effects")
            action_ctx:stay()
        end)
        actions:bind("g", "Push Mode", a.push(function(child)
            child:bind("esc", "Back", a.pop, GLOBAL)
            child:bind("s", "Nested Shell", a.shell("echo nested"))
        end, "Pushed"))
    end, { capture = true })

    if ctx:app_matches("Finder") then
        menu:bind("shift+cmd+f", "Finder Home", a.open("~"), GLOBAL)
    end
end
