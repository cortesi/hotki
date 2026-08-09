eguidev.configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function assert_clean_layout(vp: Viewport, label: string)
    local issues = vp:layout_issues()
    assert(#issues == 0, label .. " should have no layout issues")
end

local function resize_and_check(vp: Viewport, width: number, height: number, label: string)
    vp:resize({ inner_size = { x = width, y = height } })
    vp:wait(function(state)
        return state ~= nil
            and math.abs(state.inner_size.x - width) <= 2
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

eguidev.fixture("hotki.main.ready_empty")
local main = eguidev.wait_viewport({ name = "main" })
assert(assert(main:state()).title == "Hotki", "main window should use the native Hotki title")
eguidev.widget("main.activity.empty"):expect({
    label = "No recent activity.",
    visible = true,
})
eguidev.widget("main.footer.primary"):expect({ label = "Reload Config", role = "button" })
eguidev.widget("main.footer.logs"):expect({ label = "Show Logs", role = "button" })
eguidev.widget("main.notice"):expect({ present = false })
eguidev.widget("details.root"):expect({ present = false })
eguidev.widget("main.tabs"):expect({ present = false })
eguidev.widget("main.config"):expect({ present = false })
resize_and_check(main, 440, 280, "ready main window at minimum size")
resize_and_check(main, 900, 600, "ready main window at expanded size")

eguidev.fixture("hotki.main.populated")
main = eguidev.wait_viewport({ name = "main" })
eguidev.widget("main.activity.0.title"):expect({ label = "Config reloaded" })
eguidev.widget("main.activity.0.text"):expect({
    label = "The new configuration is active.",
    role = "selectable",
})
eguidev.widget("main.activity.1.title"):expect({ label = "Earlier activity" })
eguidev.widget("main.activity.1.text"):expect({
    label = "This row should follow the newest activity.",
    role = "selectable",
})
local first_time = assert(eguidev.widget("main.activity.0.time"):state()).value_text
assert(string.match(first_time, "^%d%d:%d%d$") ~= nil, "activity time should use local HH:MM")
eguidev.wait_frames(3)
assert(
    assert(eguidev.widget("main.activity.0.time"):state()).value_text == first_time,
    "activity time should remain static"
)
resize_and_check(main, 440, 280, "populated main window at minimum size")
resize_and_check(main, 900, 600, "populated main window at expanded size")

eguidev.fixture("hotki.main.permission_required")
main = eguidev.wait_viewport({ name = "main" })
eguidev.widget("main.notice.title"):expect({ label = "Hotki needs permission" })
eguidev.widget("main.notice.detail"):expect({
    label = "Grant Accessibility and Input Monitoring in System Settings.",
})
eguidev.widget("main.notice.tone"):expect({ value = "attention" })
eguidev.widget("main.notice.progress"):expect({ value = false })
eguidev.widget("main.footer.primary"):expect({ label = "Open Permissions" })
resize_and_check(main, 440, 280, "permission main window at minimum size")
resize_and_check(main, 900, 600, "permission main window at expanded size")

eguidev.fixture("hotki.main.invalid_config")
main = eguidev.wait_viewport({ name = "main" })
eguidev.widget("main.notice.title"):expect({
    label = "Hotki couldn't load the configuration",
})
eguidev.widget("main.notice.detail"):expect({
    label = "The previous configuration is still active.",
})
eguidev.widget("main.notice.tone"):expect({ value = "error" })
eguidev.widget("main.footer.primary"):expect({ label = "Try Again" })
eguidev.widget("main.activity.0.text"):expect({
    label_contains = "config-with-a-very-long-name-that-wraps.luau",
    role = "selectable",
})
resize_and_check(main, 440, 280, "invalid-config main window at minimum size")
resize_and_check(main, 900, 600, "invalid-config main window at expanded size")

eguidev.fixture("hotki.main.ready_empty")
main = eguidev.wait_viewport({ name = "main" })
local main_id = main.id
local main_position = assert(main:state()).outer_pos
eguidev.widget("main.footer.logs"):click()
local logs = eguidev.wait_viewport({ name = "logs" })
assert(assert(logs:state()).title == "Hotki Logs", "logs window should use its diagnostic title")
eguidev.widget("logs.clear"):expect({ label = "Clear", role = "button" })
eguidev.widget("logs.count"):expect({ present = false })
assert_clean_layout(logs, "logs window")
assert(eguidev.wait_viewport({ name = "main" }).id == main_id, "opening logs should keep the main viewport")
assert(
    positions_match(assert(eguidev.wait_viewport({ name = "main" }):state()).outer_pos, main_position),
    "opening logs should not move main"
)
local logs_id = logs.id
eguidev.widget("main.footer.logs"):click()
assert(eguidev.wait_viewport({ name = "logs" }).id == logs_id, "reopening logs should reuse one viewport")
local logs_count = 0
for _, vp in ipairs(eguidev.viewports()) do
    local state = vp:state()
    if state ~= nil and state.name == "logs" then
        logs_count += 1
    end
end
assert(logs_count == 1, "only one logs viewport should exist")
eguidev.widget("logs.clear"):click()
eguidev.widget("logs.empty"):expect({ label = "No logs yet." })

return {
    main = main_id,
    logs = logs_id,
}
