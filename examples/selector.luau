themes:use("default")

hotki.root(function(menu, ctx)
    menu:bind("shift+cmd+0", "Run Application", action.selector({
        title = "Run Application",
        placeholder = "Search apps...",
        items = hotki.applications,
        on_select = function(actx: ActionContext, item: any, query: string)
            actx:exec(action.open(item.data.path))
        end,
    }))
end)
