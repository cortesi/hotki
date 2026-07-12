local actions = {
    pop = function(ctx)
        ctx:pop()
    end,
    exit = function(ctx)
        ctx:exit()
    end,
    show_root = function(ctx)
        ctx:show_root()
    end,
    hide_hud = function(ctx)
        ctx:hide_hud()
    end,
    reload_config = function(ctx)
        ctx:reload_config()
    end,
    clear_notifications = function(ctx)
        ctx:clear_notifications()
    end,
    stay = function(ctx)
        ctx:stay()
    end,
    notify = function(kind, title, body)
        return function(ctx)
            ctx:notify(kind, title, body)
        end
    end,
    push = function(render, title)
        return function(ctx)
            ctx:push(render, title)
        end
    end,
    shell = function(cmd, opts)
        return function(ctx)
            ctx:shell(cmd, opts)
        end
    end,
    open = function(target)
        return function(ctx)
            ctx:open(target)
        end
    end,
    relay = function(spec)
        return function(ctx)
            ctx:relay(spec)
        end
    end,
    show_details = function(toggle)
        return function(ctx)
            ctx:show_details(toggle)
        end
    end,
    set_volume = function(level)
        return function(ctx)
            ctx:set_volume(level)
        end
    end,
    change_volume = function(delta)
        return function(ctx)
            ctx:change_volume(delta)
        end
    end,
    mute = function(toggle)
        return function(ctx)
            ctx:mute(toggle)
        end
    end,
    hold = function(action, opts)
        return function(ctx)
            ctx:until_keyup(action, opts)
        end
    end,
    select = function(spec)
        return function(ctx)
            ctx:select(spec)
        end
    end,
}

return table.freeze(actions)
