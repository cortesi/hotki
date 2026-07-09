local function repeat_action(act: Action, opts: RepeatOptions): Action
    return function(actx: ActionContext)
        actx:until_keyup(act, opts)
    end
end

type BindingEntry = {
    chord: string,
    desc: string,
    action: Action,
}

hotki.root(function(menu, ctx)
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
        root:bind("v", "Set Volume 50%", action.set_volume(50), { stay = true })
        root:bind("shift+V", "Change Volume +10", action.change_volume(10), { stay = true })
        root:bind("m", "Mute", action.mute("on"), { stay = true })
        root:bind("u", "Unmute", action.mute("off"), { stay = true })
        root:bind("shift+M", "Toggle Mute", action.mute("toggle"), { stay = true })
        root:bind("x", "Complex Action", function(actx)
            actx:shell("echo hello")
            actx:notify("success", "Done", "Queued a shell command + notification")
            actx:stay()
        end)
        root:bind("g", "Global Key", action.shell("echo 'Global'"), { global = true })
        root:bind("shift+X", "Hidden Key", action.shell("echo 'Hidden'"), { hidden = true })
        root:bind("a", "Repeat Shell", repeat_action(action.shell("echo tick"), {
            delay_ms = 250,
            interval_ms = 350,
        }))
        root:bind("b", "Repeat Relay", repeat_action(action.relay("cmd+c"), {
            delay_ms = 250,
            interval_ms = 350,
        }))
        local bindings: { BindingEntry } = {
            { chord = "p", desc = "Loop Pop", action = action.pop },
            { chord = "shift+P", desc = "Loop Handler", action = function(actx)
                actx:notify("info", "Batch", "Handler action ran")
            end },
        }
        for _, binding in bindings do
            root:bind(binding.chord, binding.desc, binding.action)
        end
        root:submenu("o", "Loop Submenu", function(loop_menu, loop_ctx)
            loop_menu:bind("x", "Nested Shell", action.shell("echo nested"))
        end)
        root:submenu("n", "Notifications", function(sub, subctx)
            sub:bind("s", "Success", action.shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            sub:bind("i", "Info", action.shell("echo info", { ok_notify = "info", err_notify = "warn" }), { stay = true })
            sub:bind("w", "Warning", action.shell("echo warn && false"), { stay = true })
            sub:bind("e", "Error", action.shell("echo error && false", { ok_notify = "ignore", err_notify = "error" }), { stay = true })
        end)
    end, { capture = true })
end)
