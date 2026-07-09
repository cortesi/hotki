local demo_items: { SelectorItem<string> } = {
    { label = "Calendar", sublabel = "demo://calendar", data = "calendar" },
    { label = "Calculator", sublabel = "demo://calculator", data = "calculator" },
    { label = "Callsheet", sublabel = "demo://callsheet", data = "callsheet" },
}

local function exit_action(actx: ActionContext)
    actx:exit()
end

local function pop(actx: ActionContext)
    actx:pop()
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

hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Back", pop, { global = true, hidden = true })
    end

    menu:submenu("shift+cmd+0", "activate", function(root, inner)
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

        root:submenu("c", "Key Capture", function(sub, subctx)
            sub:bind("a", "Hi", shell("echo hi", { ok_notify = "success", err_notify = "warn" }))
        end, { capture = true })

        root:submenu("s", "Shell", function(sub, subctx)
            sub:bind("e", "Simple echo", shell("echo hi", { ok_notify = "success", err_notify = "warn" }))
            sub:bind("s", "Sleep 5s", shell("sleep 5; echo slept 5s", { ok_notify = "success", err_notify = "warn" }))
            sub:bind("r", "Repeat Shell Default", repeat_action(shell("echo tick", { ok_notify = "success", err_notify = "warn" }), {
                delay_ms = 250,
                interval_ms = 350,
            }))
            sub:bind("t", "Repeat Shell Custom", repeat_action(shell("echo tock", { ok_notify = "success", err_notify = "warn" }), {
                delay_ms = 250,
                interval_ms = 350,
            }))
        end)

        root:submenu("r", "Relay", function(sub, subctx)
            sub:bind("a", "Send X", relay("shift+x"), { stay = true })
            sub:bind("b", "Repeat Relay Custom", repeat_action(relay("shift+x"), {
                delay_ms = 200,
                interval_ms = 300,
            }))
        end)

        root:submenu("m", "Media", function(sub, subctx)
            sub:bind("p", "Play/Pause", shell("osascript -e 'tell application \"Spotify\" to playpause'"))
            sub:bind("n", "Next", shell("osascript -e 'tell application \"Spotify\" to next track'"))
            sub:bind("b", "Previous", shell("osascript -e 'tell application \"Spotify\" to previous track'"))
            sub:bind("1", "10%", set_volume(10), { stay = true })
            sub:bind("5", "50%", set_volume(50), { stay = true })
            sub:bind("m", "mute", mute("toggle"), { stay = true })
            sub:bind(
                "up", "Up 10", repeat_action(change_volume(10), {
                    delay_ms = 250,
                    interval_ms = 350,
                }), { stay = true }
            )
            sub:bind(
                "down", "Down 10", repeat_action(change_volume(-10), {
                    delay_ms = 250,
                    interval_ms = 350,
                }), { stay = true }
            )
        end)

        root:submenu("n", "Notifications", function(sub, subctx)
            sub:bind("s", "Success", shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            sub:bind("i", "Info", shell("echo info", { ok_notify = "info", err_notify = "warn" }), { stay = true })
            sub:bind("w", "Warning", shell("echo warn && false"), { stay = true })
            sub:bind("e", "Error", shell("echo error && false", { ok_notify = "ignore", err_notify = "error" }), { stay = true })
            sub:bind("a", "Run Application", function(actx)
                actx:select({
                    title = "Run Application",
                    placeholder = "Search apps...",
                    items = hotki.applications,
                    on_select = function(select_ctx: ActionContext, item: SelectorItem<ApplicationInfo>, query: string)
                        select_ctx:open(item.data.path)
                    end,
                })
            end)
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
            sub:submenu("m", "Mini", function(mini, mini_ctx)
                mini:bind("s", "Success", shell("echo success", { ok_notify = "success", err_notify = "warn" }), { stay = true })
            end)
        end)

        root:bind("shift+cmd+0", "exit", exit_action, { global = true, hidden = true })
    end)
end)
