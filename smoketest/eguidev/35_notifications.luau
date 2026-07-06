configure({ timeout_ms = 15000, poll_interval_ms = 25 })

fixture("hotki.notifications.variants")
wait_for_frames(5)

local expected = {
    ["notification.stack.item.0"] = "success",
    ["notification.stack.item.1"] = "error",
    ["notification.stack.item.2"] = "warn",
    ["notification.stack.item.3"] = "info",
}
local seen = {}
local live_ids = {}

local stack_count = expect("notification.stack.count", { viewport = "root", value_text_contains = "4" }).value_text
assert(widget("notification.stack", { viewport = "root" }):state() ~= nil)

for base, _ in pairs(expected) do
    seen[base] = widget(base .. ".kind", { viewport = "root" }):state().value_text
    local live_id = widget(base .. ".live_id", { viewport = "root" }):state().value_text
    assert(live_id ~= "")
    table.insert(live_ids, live_id)
end

for base, kind in pairs(expected) do
    assert(seen[base] == kind, base .. " should expose kind " .. kind)
end

local function widget_value(id: string, viewport: string): string
    return widget(id, { viewport = viewport }):state().value_text
end

local function rect_width(rect: Rect): number
    return rect.max.x - rect.min.x
end

local rects = {}
for _, live_id in ipairs(live_ids) do
    local body_widget = widget(live_id .. ".body", { viewport = live_id })
    local body_state = body_widget:state()
    local body_measure = body_widget:text_measure() :: any
    local rect = {
        id = live_id,
        current_x = tonumber(widget_value(live_id .. ".current_x", live_id)),
        current_y = tonumber(widget_value(live_id .. ".current_y", live_id)),
        target_x = tonumber(widget_value(live_id .. ".target_x", live_id)),
        expected_x = tonumber(widget_value(live_id .. ".expected_x", live_id)),
        target_y = tonumber(widget_value(live_id .. ".target_y", live_id)),
        side = widget_value(live_id .. ".side", live_id),
        width = tonumber(widget_value(live_id .. ".width", live_id)),
        body_width = rect_width(body_state.rect),
        body_wrap_width = tonumber(widget_value(live_id .. ".body_wrap_width", live_id)),
        body_text = body_state.value_text,
        body_visible_text = body_measure.visible_text,
        body_ellipsis = body_measure.ellipsis,
        height = tonumber(widget_value(live_id .. ".height", live_id)),
    }
    assert(rect.current_x ~= nil and rect.current_y ~= nil, live_id .. " should expose current position")
    assert(rect.target_x ~= nil and rect.target_y ~= nil, live_id .. " should expose target position")
    assert(rect.expected_x ~= nil, live_id .. " should expose expected x")
    assert(rect.width ~= nil and rect.width > 0, live_id .. " should expose positive width")
    assert(rect.body_wrap_width ~= nil and rect.body_wrap_width > 0, live_id .. " should expose body width")
    assert(rect.height ~= nil and rect.height > 0, live_id .. " should expose positive height")
    assert(rect.target_x >= 0 and rect.target_y >= 0, live_id .. " target should be on-screen")
    assert(rect.side == "left", live_id .. " should use left-side notification config")
    assert(math.abs(rect.target_x - rect.expected_x) <= 1, live_id .. " should target its configured side")
    assert(rect.body_width <= rect.body_wrap_width + 1, live_id .. " body should stay within wrap width")
    assert(rect.body_visible_text == rect.body_text, live_id .. " body text should be fully visible")
    assert(rect.body_ellipsis == false, live_id .. " body text should not ellipsize")
    table.insert(rects, rect)
end

for i = 1, #rects do
    for j = i + 1, #rects do
        local a = rects[i]
        local b = rects[j]
        local separated =
            a.target_x + a.width <= b.target_x or
            b.target_x + b.width <= a.target_x or
            a.target_y + a.height <= b.target_y or
            b.target_y + b.height <= a.target_y
        assert(separated, a.id .. " should not overlap " .. b.id)
    end
end

return {
    stack_count = stack_count,
    kinds = seen,
    live_count = #live_ids,
}
