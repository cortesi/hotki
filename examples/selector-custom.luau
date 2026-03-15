themes:use("default")

local items = {
    "Alpha",
    "Beta",
    "Gamma",
    { label = "Delta", sublabel = "Custom sublabel", data = "4" },
    { label = "Epsilon", data = "ep" },
    { label = "Zeta", data = "ze" },
}

hotki.root(function(menu, ctx)
    menu:bind("shift+cmd+0", "Pick Item", action.selector({
        title = "Pick Item",
        placeholder = "Type to filter...",
        items = items,
        on_select = function(actx: ActionContext, item: any, query: string)
            actx:notify("info", "Selected", item.label .. " (query: " .. query .. ")")
        end,
        on_cancel = function(actx: ActionContext)
            actx:notify("info", "Selector", "Cancelled")
        end,
    }))
end)
