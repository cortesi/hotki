local function pop(actx: ActionContext)
    actx:pop()
end

local function exit(actx: ActionContext)
    actx:exit()
end

local function shell(cmd: string, opts: ShellOptions?): Action
    return function(actx: ActionContext)
        actx:shell(cmd, opts)
    end
end

local function relay(spec: string): Action
    return function(actx: ActionContext)
        actx:relay(spec)
    end
end

local function show_details(toggle: Toggle): Action
    return function(actx: ActionContext)
        actx:show_details(toggle)
    end
end

local function set_volume(level: number): Action
    return function(actx: ActionContext)
        actx:set_volume(level)
    end
end

local function change_volume(delta: number): Action
    return function(actx: ActionContext)
        actx:change_volume(delta)
    end
end

local function mute(toggle: Toggle): Action
    return function(actx: ActionContext)
        actx:mute(toggle)
    end
end

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
        menu:bind("esc", "Back", pop, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "Main", function(root, inner)
        root:bind("shift+cmd+0", "Exit", exit, { global = true, hidden = true })
        root:bind("s", "Shell Command", shell("echo 'Hello'"))
        root:bind("shift+S", "Shell with Notifications", shell("echo 'Success'", { ok_notify = "success", err_notify = "error" }))
        root:bind("r", "Relay Keystroke", relay("cmd+c"))
        root:bind("c", "Clear Notifications", function(actx)
            actx:clear_notifications()
        end)
        root:bind("d", "Toggle Details", show_details("toggle"), { stay = true })
        root:bind("shift+R", "Reload Config", function(actx)
            actx:reload_config()
        end)
        root:bind("h", "Show Root HUD", function(actx)
            actx:show_root()
        end)
        root:bind("shift+H", "Hide HUD", function(actx)
            actx:hide_hud()
        end)
        root:bind("v", "Set Volume 50%", set_volume(50), { stay = true })
        root:bind("shift+V", "Change Volume +10", change_volume(10), { stay = true })
        root:bind("m", "Mute", mute("on"), { stay = true })
        root:bind("u", "Unmute", mute("off"), { stay = true })
        root:bind("shift+M", "Toggle Mute", mute("toggle"), { stay = true })
        root:bind("x", "Complex Action", function(actx)
            actx:shell("echo hello")
            actx:notify("success", "Done", "Queued a shell command + notification")
            actx:stay()
        end)
        root:bind("g", "Global Key", shell("echo 'Global'"), { global = true })
        root:bind("shift+X", "Hidden Key", shell("echo 'Hidden'"), { hidden = true })
        root:bind("a", "Repeat Shell", repeat_action(shell("echo tick"), {
            delay_ms = 250,
            interval_ms = 350,
        }))
        root:bind("b", "Repeat Relay", repeat_action(relay("cmd+c"), {
            delay_ms = 250,
            interval_ms = 350,
        }))
        local bindings: { BindingEntry } = {
            { chord = "p", desc = "Loop Pop", action = pop },
            { chord = "shift+P", desc = "Loop Handler", action = function(actx)
                actx:notify("info", "Batch", "Handler action ran")
            end },
        }
        for _, binding in bindings do
            root:bind(binding.chord, binding.desc, binding.action)
        end
        root:submenu("o", "Loop Submenu", function(loop_menu, loop_ctx)
            loop_menu:bind("x", "Nested Shell", shell("echo nested"))
        end)
        root:submenu("n", "Notifications", function(sub, subctx)
            sub:bind("s", "Success", shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            sub:bind("i", "Info", shell("echo info", { ok_notify = "info", err_notify = "warn" }), { stay = true })
            sub:bind("w", "Warning", shell("echo warn && false"), { stay = true })
            sub:bind("e", "Error", shell("echo error && false", { ok_notify = "ignore", err_notify = "error" }), { stay = true })
        end)
    end, { capture = true })
end)
