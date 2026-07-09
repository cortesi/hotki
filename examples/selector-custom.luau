local items: { SelectorItem<string> } = {
    { label = "Alpha", data = "Alpha" },
    { label = "Beta", data = "Beta" },
    { label = "Gamma", data = "Gamma" },
    { label = "Delta", sublabel = "Custom sublabel", data = "4" },
    { label = "Epsilon", data = "ep" },
    { label = "Zeta", data = "ze" },
}

hotki.root(function(menu, ctx)
    menu:bind("shift+cmd+0", "Pick Item", function(actx)
        actx:select({
            title = "Pick Item",
            placeholder = "Type to filter...",
            items = items,
            on_select = function(select_ctx: ActionContext, item: SelectorItem<string>, query: string)
                select_ctx:notify("info", "Selected", item.label .. " (query: " .. query .. ")")
            end,
            on_cancel = function(select_ctx: ActionContext)
                select_ctx:notify("info", "Selector", "Cancelled")
            end,
        })
    end)
end)
