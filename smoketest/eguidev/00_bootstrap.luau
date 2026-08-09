eguidev.configure({ timeout_ms = 10000, poll_interval_ms = 25 })

local expected = {
    ["hotki.basic.default"] = false,
    ["hotki.main.ready_empty"] = false,
    ["hotki.main.populated"] = false,
    ["hotki.main.permission_required"] = false,
    ["hotki.main.invalid_config"] = false,
    ["hotki.logs"] = false,
    ["hotki.tray.about"] = false,
    ["hotki.hud"] = false,
    ["hotki.hud.pressed"] = false,
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
    ["hotki.permissions.hidden_missing"] = false,
    ["hotki.permissions.hidden_recovered"] = false,
}

for _, fixture_info in ipairs(eguidev.fixtures()) do
    if expected[fixture_info.name] ~= nil then
        expected[fixture_info.name] = true
        assert(#fixture_info.ready >= 1, fixture_info.name .. " should advertise ready conditions")
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
    for _, viewport in ipairs(eguidev.viewports()) do
        for _, widget in ipairs(viewport:widgets()) do
            local key = viewport.id .. ":" .. widget.id
            assert(seen[key] == nil, "duplicate widget id in viewport: " .. key)
            seen[key] = true
        end
    end
end

eguidev.fixture("hotki.basic.default")
local root_vp = eguidev.root
local root_viewport_state = assert(root_vp:state())
assert(root_viewport_state.title == "hotki-dev", "automation must use a distinct native identity")
eguidev.widget("app.ready"):expect({ value_text_contains = "true" })
local settle_report = root_vp:settle()
local saw_app_idle = false
for _, phase in ipairs(settle_report.phases) do
    if phase.phase == "app_idle" then
        saw_app_idle = true
        assert(phase.complete, "hotki idle provider should report idle")
    end
end
assert(saw_app_idle, "hotki should expose app_idle settle phase")
assert_no_duplicate_widget_ids()

local root_state = assert(eguidev.widget("main.root"):expect({ visible = true }))
assert(root_state.visible, "main.root should be visible after baseline fixture")

return {
    app_ready = assert(eguidev.widget("app.ready"):state()).value_text,
    viewports = #eguidev.viewports(),
}
