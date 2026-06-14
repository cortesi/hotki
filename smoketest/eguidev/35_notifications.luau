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
local stack_count = nil

for _, viewport in ipairs(viewports()) do
    local stack_ok = pcall(function()
        stack_count = viewport:widget_get("notification.stack.count"):state().value_text
    end)
    if stack_ok and stack_count == "4" then
        assert(viewport:widget_get("notification.stack"):state() ~= nil)
    end

    for base, _ in pairs(expected) do
        local item_ok = pcall(function()
            seen[base] = viewport:widget_get(base .. ".kind"):state().value_text
            local live_id = viewport:widget_get(base .. ".live_id"):state().value_text
            assert(live_id ~= "")
            table.insert(live_ids, live_id)
        end)
        if item_ok then
            seen[base] = viewport:widget_get(base .. ".kind"):state().value_text
        end
    end
end

assert(stack_count == "4", "notification stack count should reflect all live variants")
for base, kind in pairs(expected) do
    assert(seen[base] == kind, base .. " should expose kind " .. kind)
end

local function widget_value(id: string): string
    for _, viewport in ipairs(viewports()) do
        local ok, value = pcall(function()
            return viewport:widget_get(id):state().value_text
        end)
        if ok then
            return value
        end
    end
    error("missing widget " .. id)
end

local rects = {}
for _, live_id in ipairs(live_ids) do
    local rect = {
        id = live_id,
        current_x = tonumber(widget_value(live_id .. ".current_x")),
        current_y = tonumber(widget_value(live_id .. ".current_y")),
        target_x = tonumber(widget_value(live_id .. ".target_x")),
        target_y = tonumber(widget_value(live_id .. ".target_y")),
        width = tonumber(widget_value(live_id .. ".width")),
        height = tonumber(widget_value(live_id .. ".height")),
    }
    assert(rect.current_x ~= nil and rect.current_y ~= nil, live_id .. " should expose current position")
    assert(rect.target_x ~= nil and rect.target_y ~= nil, live_id .. " should expose target position")
    assert(rect.width ~= nil and rect.width > 0, live_id .. " should expose positive width")
    assert(rect.height ~= nil and rect.height > 0, live_id .. " should expose positive height")
    assert(rect.target_x >= 0 and rect.target_y >= 0, live_id .. " target should be on-screen")
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
