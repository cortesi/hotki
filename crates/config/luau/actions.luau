local function relay(spec)
    return function(ctx)
        ctx:relay(spec)
    end
end

local function relay_to_app(app_name)
    return function(spec)
        return function(ctx)
            ctx:relay_to_app(app_name, spec)
        end
    end
end

local function strip_plus(value)
    while value:sub(1, 1) == "+" do
        value = value:sub(2)
    end
    while value:sub(-1) == "+" do
        value = value:sub(1, -2)
    end
    return value
end

local function launch_application(options)
    local options = options or {}
    return function(ctx)
        ctx:select({
            title = options.title or "Run Application",
            placeholder = options.placeholder or "Search apps...",
            items = hotki.applications,
            max_visible = options.max_visible,
            on_select = function(select_ctx, item, query)
                select_ctx:open(item.data.path)
            end,
        })
    end
end

local function relay_with(prefix)
    local prefix = strip_plus(prefix)
    return function(spec)
        local spec = strip_plus(spec)
        return relay(prefix .. "+" .. spec)
    end
end

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
    exec = function(spec)
        return function(ctx)
            ctx:exec(spec)
        end
    end,
    open = function(target)
        return function(ctx)
            ctx:open(target)
        end
    end,
    relay = relay,
    relay_to_app = relay_to_app,
    relay_with = relay_with,
    launch_application = launch_application,
    show_main_window = function(toggle)
        return function(ctx)
            ctx:show_main_window(toggle)
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
