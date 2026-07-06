configure({ timeout_ms = 10000, poll_interval_ms = 25 })

local function widget_size(viewport: Viewport, id: string): { width: number, height: number }
    local rect = viewport:widget_get(id):state().rect
    return {
        width = rect.max.x - rect.min.x,
        height = rect.max.y - rect.min.y,
    }
end

fixture("hotki.details")
local details = viewport({ name = "details" })
wait_for_frames(5)
local first_size = widget_size(details, "details.root")

for _ = 1, 3 do
    fixture("hotki.details")
    wait_for_frames(5)
    details = viewport({ name = "details" })
    local next_size = widget_size(details, "details.root")
    assert(math.abs(next_size.width - first_size.width) <= 2, "details width should not grow on reopen")
    assert(math.abs(next_size.height - first_size.height) <= 2, "details height should not grow on reopen")
end

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

details:widget_get("details.tab.notifications"):click()
assert(details:widget_get("details.active_tab"):state().value_text == "notifications")
details:wait_for_widget("details.notification.0.title", function(widget)
    return widget.value_text == "Config"
end)
assert(details:widget_get("details.notification.0.row"):state() ~= nil)
details:wait_for_widget("details.notification.0.text", function(widget)
    return widget.value_text == "Reloaded successfully"
end)

fixture("hotki.notifications")
details = viewport({ name = "details" })
details:wait_for_widget("details.notification.0.title", function(widget)
    return widget.value_text == "Eguidev"
end)

assert(root():widget_get("notification.stack.count"):state().value_text == "1")
assert(root():widget_get("notification.stack.item.0.title"):state().value_text == "Eguidev")
root():wait_for_widget("notification.stack.item.0.live_id", function(widget)
    return widget.value_text ~= ""
end)
local live_base = root():widget_get("notification.stack.item.0.live_id"):state().value_text
local live_notification = viewport({ name = live_base })
assert(live_notification:widget_get(live_base .. ".kind"):state().value_text == "info")
assert(tonumber(live_notification:widget_get(live_base .. ".width"):state().value_text) > 0)
assert(tonumber(live_notification:widget_get(live_base .. ".height"):state().value_text) > 0)

return {
    active_viewport = details.id,
    live_notification = live_base,
}
