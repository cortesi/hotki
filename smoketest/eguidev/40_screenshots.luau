eguidev.configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function assert_image_ref(image: any, label: string)
    assert(image.type == "image_ref", label .. " should return an image_ref")
end

eguidev.fixture("hotki.main.ready_empty")
eguidev.widget("main.root"):expect({ visible = true })
eguidev.widget("main.activity.empty"):expect({ visible = true })

eguidev.fixture("hotki.main.populated")
eguidev.widget("main.activity.0.title"):expect({ label = "Config reloaded" })

eguidev.fixture("hotki.main.permission_required")
eguidev.widget("main.notice.title"):expect({ label = "Hotki needs permission" })

eguidev.fixture("hotki.main.invalid_config")
eguidev.widget("main.activity.0.title"):expect({ label = "Configuration error" })

eguidev.fixture("hotki.logs")
eguidev.widget("logs.0.message"):expect({ visible = true })

eguidev.fixture("hotki.permissions.mixed")
eguidev.widget("permissions.root"):expect({ visible = true })

eguidev.fixture("hotki.hud")
eguidev.widget("hud.panel"):expect({ visible = true })

eguidev.fixture("hotki.hud.demo")
eguidev.fixture("hotki.selector")
eguidev.widget("selector.panel"):expect({ visible = true })

local previous_live_id_state = eguidev.widget("notification.stack.item.0.live_id"):state()
local previous_live_id = if previous_live_id_state == nil then nil else previous_live_id_state.value_text
eguidev.fixture("hotki.notifications")
local live_id_state = eguidev.widget("notification.stack.item.0.live_id"):wait(function(state)
    return state ~= nil and state.value_text ~= "" and state.value_text ~= previous_live_id
end)
local live_id = assert(live_id_state).value_text
eguidev.widget(live_id .. ".title"):expect({ value_text_contains = "Eguidev" })
local screenshot_image = eguidev.root:screenshot()
assert_image_ref(screenshot_image, "root screenshot")

return {
    image = screenshot_image,
    main_states_checked = true,
    logs_checked = true,
}
