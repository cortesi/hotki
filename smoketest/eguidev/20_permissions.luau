eguidev.configure({ timeout_ms = 10000, poll_interval_ms = 25 })

local function require_viewport(name: string): Viewport
    local found = eguidev.viewports({ name = name })[1]
    if found == nil then
        eguidev.wait(function()
            found = eguidev.viewports({ name = name })[1]
            return found ~= nil
        end)
    end
    return assert(found, name .. " viewport should exist")
end

local function widget_in(viewport: Viewport, id: string)
    local found: any = nil
    local function discover(): boolean
        for _, candidate in ipairs(viewport:widgets({ id_prefix = id })) do
            if candidate.id == id then
                found = candidate
                return true
            end
        end
        return false
    end
    if not discover() then
        eguidev.wait(discover)
    end
    return assert(found, "missing widget " .. id .. " in viewport " .. viewport.id)
end

eguidev.fixture("hotki.permissions.none_granted")
local permissions = require_viewport("permissions")

widget_in(permissions, "permissions.description"):wait({ visible = true })
local accessibility = assert(widget_in(permissions, "permissions.accessibility.status"):state())
local input_monitoring = assert(widget_in(permissions, "permissions.input_monitoring.status"):state())

assert(accessibility.value_text == "Not granted", "accessibility should be missing")
assert(input_monitoring.value_text == "Not granted", "input monitoring should be missing")
assert(assert(widget_in(permissions, "permissions.accessibility.granted"):state()).value == false)
assert(assert(widget_in(permissions, "permissions.input_monitoring.granted"):state()).value == false)
assert(assert(widget_in(permissions, "permissions.accessibility.open_settings"):state()).role == "button")
assert(assert(widget_in(permissions, "permissions.input_monitoring.open_settings"):state()).role == "button")

widget_in(permissions, "permissions.accessibility.open_settings"):click()
widget_in(permissions, "permissions.accessibility.open_intent"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)

eguidev.fixture("hotki.permissions.all_granted")
permissions = require_viewport("permissions")
widget_in(permissions, "permissions.accessibility.granted"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
widget_in(permissions, "permissions.input_monitoring.granted"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)

eguidev.fixture("hotki.permissions.mixed")
permissions = require_viewport("permissions")
widget_in(permissions, "permissions.accessibility.granted"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
widget_in(permissions, "permissions.input_monitoring.granted"):wait(function(widget)
    return widget ~= nil and widget.value == false
end)

return {
    accessibility = accessibility.value_text,
    input_monitoring = input_monitoring.value_text,
}
