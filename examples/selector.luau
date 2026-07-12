return function(menu, ctx)
    menu:bind("shift+cmd+0", "Run Application", function(actx)
        actx:select({
            title = "Run Application",
            placeholder = "Search apps...",
            items = hotki.applications,
            on_select = function(select_ctx: ActionContext, item: SelectorItem<ApplicationInfo>, query: string)
                select_ctx:open(item.data.path)
            end,
        })
    end)
end
