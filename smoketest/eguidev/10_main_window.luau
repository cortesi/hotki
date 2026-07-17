configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function require_viewport(name: string): Viewport
    local found = viewport({ name = name })
    assert(found ~= nil, name .. " viewport should exist")
    return found :: Viewport
end

local function assert_clean_layout(vp: Viewport, label: string)
    local issues = vp:check_layout()
    assert(#issues == 0, label .. " should have no layout issues")
end

local function resize_and_check(vp: Viewport, width: number, height: number, label: string)
    vp:set_inner_size({ x = width, y = height })
    vp:wait_for(function(state)
        return math.abs(state.inner_size.x - width) <= 2
            and math.abs(state.inner_size.y - height) <= 2
    end)
    assert_clean_layout(vp, label)
end

local function positions_match(left: Pos?, right: Pos?): boolean
    if left == nil or right == nil then
        return left == nil and right == nil
    end
    return math.abs(left.x - right.x) <= 2 and math.abs(left.y - right.y) <= 2
end

fixture("hotki.main.ready_empty")
local main = require_viewport("main")
assert(main:state().title == "Hotki", "main window should use the native Hotki title")
expect("main.activity.empty", {
    viewport = "main",
    label = "No recent activity.",
    visible = true,
})
expect("main.footer.primary", { viewport = "main", label = "Reload Config", role = "button" })
expect("main.footer.logs", { viewport = "main", label = "Show Logs", role = "button" })
expect_absent("main.notice")
expect_absent("details.root")
expect_absent("main.tabs")
expect_absent("main.config")
resize_and_check(main, 440, 280, "ready main window at minimum size")
resize_and_check(main, 900, 600, "ready main window at expanded size")

fixture("hotki.main.populated")
main = require_viewport("main")
expect("main.activity.0.title", { viewport = "main", label = "Config reloaded" })
expect("main.activity.0.text", {
    viewport = "main",
    label = "The new configuration is active.",
    role = "selectable",
})
expect("main.activity.1.title", { viewport = "main", label = "Earlier activity" })
expect("main.activity.1.text", {
    viewport = "main",
    label = "This row should follow the newest activity.",
    role = "selectable",
})
local first_time = main:widget_get("main.activity.0.time"):state().value_text
assert(string.match(first_time, "^%d%d:%d%d$") ~= nil, "activity time should use local HH:MM")
wait_for_frames(3)
assert(
    main:widget_get("main.activity.0.time"):state().value_text == first_time,
    "activity time should remain static"
)
resize_and_check(main, 440, 280, "populated main window at minimum size")
resize_and_check(main, 900, 600, "populated main window at expanded size")

fixture("hotki.main.permission_required")
main = require_viewport("main")
expect("main.notice.title", { viewport = "main", label = "Hotki needs permission" })
expect("main.notice.detail", {
    viewport = "main",
    label = "Grant Accessibility and Input Monitoring in System Settings.",
})
expect("main.notice.tone", { viewport = "main", value = "attention" })
expect("main.notice.progress", { viewport = "main", value = false })
expect("main.footer.primary", { viewport = "main", label = "Open Permissions" })
resize_and_check(main, 440, 280, "permission main window at minimum size")
resize_and_check(main, 900, 600, "permission main window at expanded size")

fixture("hotki.main.invalid_config")
main = require_viewport("main")
expect("main.notice.title", {
    viewport = "main",
    label = "Hotki couldn't load the configuration",
})
expect("main.notice.detail", {
    viewport = "main",
    label = "The previous configuration is still active.",
})
expect("main.notice.tone", { viewport = "main", value = "error" })
expect("main.footer.primary", { viewport = "main", label = "Try Again" })
expect("main.activity.0.text", {
    viewport = "main",
    label_contains = "config-with-a-very-long-name-that-wraps.luau",
    role = "selectable",
})
resize_and_check(main, 440, 280, "invalid-config main window at minimum size")
resize_and_check(main, 900, 600, "invalid-config main window at expanded size")

fixture("hotki.main.ready_empty")
main = require_viewport("main")
local main_id = main.id
local main_position = main:state().outer_pos
main:widget_get("main.footer.logs"):click()
local logs = require_viewport("logs")
assert(logs:state().title == "Hotki Logs", "logs window should use its diagnostic title")
expect("logs.clear", { viewport = "logs", label = "Clear", role = "button" })
expect_absent("logs.count")
assert_clean_layout(logs, "logs window")
assert(require_viewport("main").id == main_id, "opening logs should keep the main viewport")
assert(
    positions_match(require_viewport("main"):state().outer_pos, main_position),
    "opening logs should not move main"
)
local logs_id = logs.id
main:widget_get("main.footer.logs"):click()
assert(require_viewport("logs").id == logs_id, "reopening logs should reuse one viewport")
local logs_count = 0
for _, vp in ipairs(viewports()) do
    if vp:state().name == "logs" then
        logs_count += 1
    end
end
assert(logs_count == 1, "only one logs viewport should exist")
logs:widget_get("logs.clear"):click()
expect("logs.empty", { viewport = "logs", label = "No logs yet." })

return {
    main = main_id,
    logs = logs_id,
}
