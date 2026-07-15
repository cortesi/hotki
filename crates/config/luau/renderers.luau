local renderers = {
    combine = function(...)
        local entries = { ... }
        return function(menu, ctx)
            for _, renderer in entries do
                renderer(menu, ctx)
            end
        end
    end,
    when_app = function(app, renderer)
        return function(menu, ctx)
            if ctx.app == app then
                renderer(menu, ctx)
            end
        end
    end,
    when_app_matches = function(pattern, renderer)
        return function(menu, ctx)
            if ctx:app_matches(pattern) then
                renderer(menu, ctx)
            end
        end
    end,
}

return table.freeze(renderers)
