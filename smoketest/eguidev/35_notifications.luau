configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function widget_value(id: string, viewport_name: string): string
    return widget(id, { viewport = viewport_name }):state().value_text
end

local function widget_number(id: string, viewport_name: string): number
    local value = tonumber(widget_value(id, viewport_name))
    assert(value ~= nil, id .. " should expose a numeric value")
    return value :: number
end

local function widget_bool(id: string, viewport_name: string): boolean
    local value = widget_value(id, viewport_name)
    assert(value == "true" or value == "false", id .. " should expose a boolean value")
    return value == "true"
end

local function rect_width(rect: Rect): number
    return rect.max.x - rect.min.x
end

local function rect_height(rect: Rect): number
    return rect.max.y - rect.min.y
end

local function wait_for_stack_count(count: number): string
    return root():wait_for_widget("notification.stack.count", function(widget_state)
        return widget_state.value_text == tostring(count)
    end).value_text
end

local function live_ids(count: number): { string }
    local ids = {}
    for index = 0, count - 1 do
        local stack_item = "notification.stack.item." .. tostring(index) .. ".live_id"
        local live_id = widget(stack_item, { viewport = "root" }):state().value_text
        assert(live_id ~= "", "stack item " .. tostring(index) .. " should expose live id")
        table.insert(ids, live_id)
    end
    return ids
end

local function assert_text_fully_visible(id: string, viewport_name: string)
    local text_widget = widget(id, { viewport = viewport_name })
    local state = text_widget:state()
    local measure = text_widget:text_measure() :: any
    assert(measure.visible_text == state.value_text, id .. " text should be fully visible")
    assert(measure.ellipsis == false, id .. " text should not ellipsize")
    return state
end

local function wait_for_live_viewport(live_id: string)
    wait_until(function()
        local ok = pcall(function()
            widget(live_id .. ".title", { viewport = live_id }):state()
        end)
        return ok
    end)
end

local function assert_card_geometry(live_id: string, expected_side: string)
    wait_for_live_viewport(live_id)
    local title_state = assert_text_fully_visible(live_id .. ".title", live_id)
    local body_state = assert_text_fully_visible(live_id .. ".body", live_id)
    local icon_state = widget(live_id .. ".icon", { viewport = live_id }):state()
    local side = widget_value(live_id .. ".side", live_id)
    local current_x = widget_number(live_id .. ".current_x", live_id)
    local current_y = widget_number(live_id .. ".current_y", live_id)
    local target_x = widget_number(live_id .. ".target_x", live_id)
    local target_y = widget_number(live_id .. ".target_y", live_id)
    local expected_x = widget_number(live_id .. ".expected_x", live_id)
    local width = widget_number(live_id .. ".width", live_id)
    local height = widget_number(live_id .. ".height", live_id)
    local body_wrap_width = widget_number(live_id .. ".body_wrap_width", live_id)
    local content_height = widget_number(live_id .. ".content_height", live_id)
    local padding = widget_number(live_id .. ".padding", live_id)
    local slop = widget_number(live_id .. ".layout_slop", live_id)
    local max_height = widget_number(live_id .. ".max_height", live_id)
    local truncated = widget_bool(live_id .. ".truncated", live_id)
    local row_top = math.min(title_state.rect.min.y, icon_state.rect.min.y)

    assert(current_x ~= nil and current_y ~= nil, live_id .. " should expose current position")
    assert(target_x >= 0 and target_y >= 0, live_id .. " target should be on-screen")
    assert(side == expected_side, live_id .. " should use " .. expected_side .. " notifications")
    assert(math.abs(target_x - expected_x) <= slop, live_id .. " should target configured side")
    assert(width > 0 and height > 0, live_id .. " should expose positive card size")
    assert(max_height >= height, live_id .. " card should not exceed max height")
    assert(truncated == false, live_id .. " normal fixture should not truncate")
    assert(height <= content_height + 2 * padding + slop, live_id .. " card should stay compact")
    assert(row_top <= padding + slop, live_id .. " title row should start at card padding")
    assert(body_state.rect.max.y + padding <= height + slop, live_id .. " body should fit in card")
    assert(title_state.rect.min.x >= padding - slop, live_id .. " title should stay inside padding")
    assert(title_state.rect.max.x <= width - padding + slop, live_id .. " title should fit width")
    assert(body_state.rect.min.x >= padding - slop, live_id .. " body should stay inside padding")
    assert(body_state.rect.max.x <= width - padding + slop, live_id .. " body should fit width")
    assert(rect_width(body_state.rect) <= body_wrap_width + slop, live_id .. " body should wrap")

    return {
        id = live_id,
        target_x = target_x,
        target_y = target_y,
        width = width,
        height = height,
    }
end

local function assert_no_overlap(rects)
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
end

local function assert_variant_fixture(fixture_name: string, expected_side: string)
    fixture(fixture_name)
    root():wait_for_settle()
    local stack_count = wait_for_stack_count(4)
    assert(widget("notification.stack", { viewport = "root" }):state() ~= nil)

    local expected = {
        ["notification.stack.item.0"] = "success",
        ["notification.stack.item.1"] = "error",
        ["notification.stack.item.2"] = "warn",
        ["notification.stack.item.3"] = "info",
    }
    for base, kind in pairs(expected) do
        local seen = widget(base .. ".kind", { viewport = "root" }):state().value_text
        assert(seen == kind, base .. " should expose kind " .. kind)
    end

    local rects = {}
    for _, live_id in ipairs(live_ids(4)) do
        table.insert(rects, assert_card_geometry(live_id, expected_side))
    end
    assert_no_overlap(rects)
    return stack_count
end

local function assert_truncated_fixture()
    fixture("hotki.notifications.truncated")
    root():wait_for_settle()
    wait_for_stack_count(1)
    local live_id = live_ids(1)[1]
    wait_for_live_viewport(live_id)
    local title_state = assert_text_fully_visible(live_id .. ".title", live_id)
    local body_widget = widget(live_id .. ".body", { viewport = live_id })
    local body_state = body_widget:state()
    local body_measure = body_widget:text_measure() :: any
    local full_body = widget_value(live_id .. ".body_text", live_id)
    local height = widget_number(live_id .. ".height", live_id)
    local max_height = widget_number(live_id .. ".max_height", live_id)
    local padding = widget_number(live_id .. ".padding", live_id)
    local slop = widget_number(live_id .. ".layout_slop", live_id)
    local truncated = widget_bool(live_id .. ".truncated", live_id)

    assert(truncated, live_id .. " should report truncation")
    assert(height <= max_height + slop, live_id .. " truncated card should stay within max height")
    assert(title_state.rect.min.y >= padding - slop, live_id .. " title should stay inside card")
    assert(body_state.rect.max.y <= height - padding + slop, live_id .. " body rect should fit")
    assert(body_state.value_text ~= full_body, live_id .. " body should clip text")
    assert(
        string.sub(full_body, 1, string.len(body_state.value_text)) == body_state.value_text,
        live_id .. " visible body text should be a prefix of full body"
    )
    assert(
        body_measure.visible_text == body_state.value_text,
        live_id .. " text measure follows visible body"
    )
    assert(body_measure.ellipsis == false, live_id .. " vertical truncation should not ellipsize")

    return live_id
end

local right_count = assert_variant_fixture("hotki.notifications.variants", "right")
local left_count = assert_variant_fixture("hotki.notifications.left_variants", "left")
local truncated_id = assert_truncated_fixture()

return {
    right_count = right_count,
    left_count = left_count,
    truncated = truncated_id,
}
