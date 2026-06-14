configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function viewport_with_widget(id: string): Viewport
    for _, viewport in ipairs(viewports()) do
        local ok = pcall(function()
            viewport:widget_get(id):state()
        end)
        if ok then
            return viewport
        end
    end
    error("no viewport exposes " .. id)
end

fixture("hotki.hud")
local root_vp = root()
root_vp:wait_for_widget("app.server.connected", function(widget)
    return widget.value_text == "true"
end)
local hud = viewport_with_widget("hud.panel")
hud:wait_for_widget_visible("hud.panel")
hud:wait_for_widget("hud.mode", function(widget)
    return widget.value_text == "hud"
end)
hud:wait_for_widget("hud.visible", function(widget)
    return widget.value_text == "true"
end)
hud:wait_for_widget("hud.geometry.width", function(widget)
    return tonumber(widget.value_text) ~= nil and tonumber(widget.value_text) > 0
end)
hud:wait_for_widget("hud.row.0.desc", function(widget)
    return widget.value_text == "Theme tester"
end)
assert(hud:widget_get("hud.row.0.chord"):state().value_text == "t")
assert(hud:widget_get("hud.row.0.is_mode"):state().value_text == "true")
assert(hud:widget_get("hud.row.0.tag"):state().value_text ~= "")

fixture("hotki.hud.mini")
root_vp:wait_for_widget("app.server.connected", function(widget)
    return widget.value_text == "true"
end)
hud = viewport_with_widget("hud.panel")
hud:wait_for_widget("hud.mode", function(widget)
    return widget.value_text == "mini"
end)
hud:wait_for_widget("hud.visible", function(widget)
    return widget.value_text == "true"
end)
hud:wait_for_widget("hud.mini.title", function(widget)
    return widget.value_text == "Mini"
end)
hud:wait_for_widget("hud.geometry.width", function(widget)
    return tonumber(widget.value_text) ~= nil and tonumber(widget.value_text) > 0
end)

fixture("hotki.selector")
root_vp:wait_for_widget("app.server.connected", function(widget)
    return widget.value_text == "true"
end)
local selector = viewport_with_widget("selector.panel")
selector:wait_for_widget("selector.title", function(widget)
    return widget.value_text == "Pick Demo"
end)
assert(selector:widget_get("selector.placeholder"):state().value_text == "Search...")
assert(selector:widget_get("selector.selected_index"):state().value_text == "0")
selector:wait_for_widget("selector.item.0.label", function(widget)
    return widget.value_text == "Alpha"
end)
assert(selector:widget_get("selector.item.0.selected"):state().value_text == "true")
assert(selector:widget_get("selector.item.0.match_indices"):state().value_text == "")
selector:wait_for_widget("selector.scroll", function(widget)
    return widget.scroll_state ~= nil
end)

fixture("hotki.selector.query")
root_vp:wait_for_widget("app.server.connected", function(widget)
    return widget.value_text == "true"
end)
selector = viewport_with_widget("selector.panel")
selector:wait_for_widget("selector.query", function(widget)
    return string.find(widget.value_text, "b", 1, true) ~= nil
end)
selector:wait_for_widget("selector.item.0.label", function(widget)
    return widget.value_text == "Beta"
end)
assert(selector:widget_get("selector.item.0.selected"):state().value_text == "true")
assert(selector:widget_get("selector.selected_index"):state().value_text == "0")
assert(selector:widget_get("selector.item.0.match_indices"):state().value_text ~= "")

return {
    hud_viewport = hud.id,
    selector_viewport = selector.id,
}
