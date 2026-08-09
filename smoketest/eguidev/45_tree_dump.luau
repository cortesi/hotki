eguidev.configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function find_dump_widget(node: any, id: string): any?
    if node.id == id then
        return node
    end
    for _, child in ipairs(node.children or {}) do
        local found = find_dump_widget(child, id)
        if found ~= nil then
            return found
        end
    end
    return nil
end

local function find_viewport_widget(viewport_dump: any, id: string): any?
    for _, widget in ipairs(viewport_dump.widgets or {}) do
        local found = find_dump_widget(widget, id)
        if found ~= nil then
            return found
        end
    end
    return nil
end

local previous_live_id_state = eguidev.widget("notification.stack.item.0.live_id"):state()
local previous_live_id = if previous_live_id_state == nil then nil else previous_live_id_state.value_text
eguidev.fixture("hotki.notifications.variants")
eguidev.widget("notification.stack.item.0.live_id"):wait(function(state)
    return state ~= nil and state.value_text ~= "" and state.value_text ~= previous_live_id
end)
eguidev.wait_frames(5)
eguidev.widget("notification.stack.count"):expect({ value_text_contains = "4" })
eguidev.wait(function()
    return eguidev.diagnostic("hotki.state").notifications.live_count == 4
end)

local hotki_state = eguidev.diagnostic("hotki.state") :: any
assert(hotki_state.notifications.live_count == 4, "hotki diagnostic should expose live notifications")
assert(hotki_state.notifications.items[1].live_id ~= "", "hotki diagnostic should expose live ids")
assert(hotki_state.notifications.items[1].title ~= "", "hotki diagnostic should expose titles")

local stack_tree = eguidev.dump({
    viewport = eguidev.root,
    root = eguidev.widget("notification.stack"),
}) :: any
assert(#stack_tree.viewports == 1, "root stack dump should include one viewport")
local stack_viewport = stack_tree.viewports[1]
assert(stack_viewport.id == "root", "stack dump should target the root viewport")
assert(#stack_viewport.widgets == 1, "stack dump should be rooted at notification.stack")

local stack = stack_viewport.widgets[1]
assert(stack.id == "notification.stack", "stack root should be notification.stack")
local count = find_dump_widget(stack, "notification.stack.count")
assert(count ~= nil and count.value_text == "4", "stack dump should expose notification count")

local first_live_id = find_dump_widget(stack, "notification.stack.item.0.live_id")
assert(first_live_id ~= nil and first_live_id.value_text ~= "", "stack dump should expose live ids")

local live_id = assert(eguidev.widget("notification.stack.item.0.live_id"):state()).value_text
assert(first_live_id.value_text == live_id, "stack dump should match the current live id")
local live_body = assert(eguidev.widget(live_id .. ".body"):state())
assert(live_body.value_text ~= "", "live body should expose text")

local notification_viewport = eguidev.wait_viewport({ name = live_id })
local notification_tree = eguidev.dump({ viewport = notification_viewport, fields = "full" }) :: any
assert(#notification_tree.viewports == 1, "live notification dump should include one viewport")
assert(notification_tree.viewports[1].name == live_id, "live viewport dump should keep its name")

local body = find_viewport_widget(notification_tree.viewports[1], live_id .. ".body")
assert(body ~= nil, "live notification dump should include the body widget")
assert(body.value_text ~= "", "live notification body should expose text in dump output")

local text = eguidev.dump_text({ viewport = notification_viewport })
assert(string.find(text, "viewport " .. live_id, 1, true) ~= nil, "text dump should name viewport")
assert(string.find(text, live_id .. ".body", 1, true) ~= nil, "text dump should include body widget")

return {
    stack_count = count.value_text,
    diagnostic_count = hotki_state.notifications.live_count,
    live_viewport = live_id,
}
