configure({ timeout_ms = 10000, poll_interval_ms = 25 })

local expected = {
    ["hotki.basic.default"] = false,
    ["hotki.details"] = false,
    ["hotki.hud"] = false,
    ["hotki.hud.mini"] = false,
    ["hotki.selector"] = false,
    ["hotki.selector.query"] = false,
    ["hotki.selector.confirmed"] = false,
    ["hotki.selector.canceled"] = false,
    ["hotki.notifications"] = false,
    ["hotki.notifications.variants"] = false,
    ["hotki.permissions"] = false,
    ["hotki.permissions.all_granted"] = false,
    ["hotki.permissions.none_granted"] = false,
    ["hotki.permissions.mixed"] = false,
}

for _, fixture_info in ipairs(fixtures()) do
    if expected[fixture_info.name] ~= nil then
        expected[fixture_info.name] = true
        assert(#fixture_info.anchors >= 1, fixture_info.name .. " should advertise anchors")
        assert(
            string.find(fixture_info.description, "lane", 1, true) ~= nil,
            fixture_info.name .. " should describe its driving lane"
        )
    end
end

for name, seen in pairs(expected) do
    assert(seen, name .. " fixture should be advertised")
end

local function assert_no_duplicate_widget_ids()
    local seen = {}
    for _, viewport in ipairs(viewports()) do
        for _, widget in ipairs(viewport:widget_list({ include_invisible = true })) do
            local key = viewport.id .. ":" .. widget.id
            assert(seen[key] == nil, "duplicate widget id in viewport: " .. key)
            seen[key] = true
        end
    end
end

fixture("hotki.basic.default")
local root_vp = root()
root_vp:wait_for_widget("app.ready", function(widget)
    return widget.value_text == "true"
end)
assert_no_duplicate_widget_ids()

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

local details = viewport_with_widget("details.root")
local root_state = details:widget_get("details.root"):state()
assert(root_state.visible, "details.root should be visible after baseline fixture")

return {
    app_ready = root_vp:widget_get("app.ready"):state().value_text,
    viewports = #viewports(),
}
