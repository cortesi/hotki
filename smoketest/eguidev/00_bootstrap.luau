configure({ timeout_ms = 10000, poll_interval_ms = 25 })

local expected = {
    ["hotki.basic.default"] = false,
    ["hotki.details"] = false,
    ["hotki.details.config"] = false,
    ["hotki.details.logs"] = false,
    ["hotki.details.about"] = false,
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
assert(root_vp:state().title == "hotki-dev", "automation must use a distinct native identity")
expect("app.ready", { viewport = "root", value_text_contains = "true" })
local settle_report = root_vp:wait_for_settle()
local saw_app_idle = false
for _, phase in ipairs(settle_report.phases) do
    if phase.phase == "app_idle" then
        saw_app_idle = true
        assert(phase.complete, "hotki idle provider should report idle")
    end
end
assert(saw_app_idle, "hotki should expose app_idle settle phase")
assert_no_duplicate_widget_ids()

local root_state = expect("details.root", { viewport = "details", visible = true })
assert(root_state.visible, "details.root should be visible after baseline fixture")

return {
    app_ready = widget("app.ready", { viewport = "root" }):state().value_text,
    viewports = #viewports(),
}
