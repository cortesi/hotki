configure({ timeout_ms = 10000, poll_interval_ms = 25 })

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

fixture("hotki.details")
local details = viewport_with_widget("details.root")

assert(details:widget_get("details.active_tab"):state().value_text == "notifications")
assert(details:widget_get("details.tab.notifications"):state().role == "selectable")
assert(details:widget_get("details.tab.config"):state().role == "selectable")
assert(details:widget_get("details.tab.logs"):state().role == "selectable")
assert(details:widget_get("details.tab.about"):state().role == "selectable")

details:widget_get("details.tab.config"):click()
details:wait_for_widget_visible("details.config.reload")
assert(details:widget_get("details.active_tab"):state().value_text == "config")
assert(details:widget_get("details.config.reload"):state().role == "button")
details:widget_get("details.config.reload"):click()
details:wait_for_settle()

details:widget_get("details.tab.logs"):click()
details:wait_for_widget_visible("details.logs.clear")
assert(details:widget_get("details.active_tab"):state().value_text == "logs")
details:widget_get("details.logs.clear"):click()
details:wait_for_widget("details.logs.count", function(widget)
    return string.find(widget.value_text, "0 log rows", 1, true) ~= nil
end)

details:widget_get("details.tab.about"):click()
details:wait_for_widget("details.about.name", function(widget)
    return widget.value_text == "Hotki"
end)
assert(details:widget_get("details.active_tab"):state().value_text == "about")
assert(details:widget_get("details.about.repository"):state().role == "link")

details:widget_get("details.tab.notifications"):click({ settle = false })
wait_for_frames(5)
assert(details:widget_get("details.active_tab"):state().value_text == "notifications")
details:wait_for_widget("details.notification.0.title", function(widget)
    return widget.value_text == "Config"
end)
assert(details:widget_get("details.notification.0.row"):state() ~= nil)
details:wait_for_widget("details.notification.0.text", function(widget)
    return widget.value_text == "Reloaded successfully"
end)

fixture("hotki.notifications")
details = viewport_with_widget("details.root")
details:wait_for_widget("details.notification.0.title", function(widget)
    return widget.value_text == "Eguidev"
end)

local live_notification = nil
local live_base = nil
for _, viewport in ipairs(viewports()) do
    for _, widget in ipairs(viewport:widget_list({ id_prefix = "notification.live.", include_invisible = true })) do
        local state = widget:state()
        if string.sub(widget.id, -6) == ".title" and state.value_text == "Eguidev" then
            live_notification = viewport
            live_base = string.sub(widget.id, 1, #widget.id - 6)
        end
    end
end
assert(live_notification ~= nil, "live notification viewport should expose the fixture title")
assert(root():widget_get("notification.stack.count"):state().value_text == "1")
assert(root():widget_get("notification.stack.item.0.title"):state().value_text == "Eguidev")
assert(live_notification:widget_get(live_base .. ".kind"):state().value_text == "info")
assert(tonumber(live_notification:widget_get(live_base .. ".width"):state().value_text) > 0)
assert(tonumber(live_notification:widget_get(live_base .. ".height"):state().value_text) > 0)

return {
    active_viewport = details.id,
    live_notification = live_base,
}
