configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function assert_image_ref(image: any, label: string)
    assert(image.type == "image_ref", label .. " should return an image_ref")
end

fixture("hotki.details")
expect("details.root", { viewport = "details", visible = true })

fixture("hotki.permissions.mixed")
expect("permissions.root", { viewport = "permissions", visible = true })

fixture("hotki.hud")
expect("hud.panel", { viewport = "hud", visible = true })

fixture("hotki.hud.demo")
fixture("hotki.selector")
expect("selector.panel", { viewport = "selector", visible = true })

fixture("hotki.notifications")
wait_until(function()
    return widget("notification.stack.item.0.live_id", { viewport = "root" }):state().value_text ~= ""
end)
local live_id = widget("notification.stack.item.0.live_id", { viewport = "root" }):state().value_text
expect(live_id .. ".title", { viewport = live_id, value_text_contains = "Eguidev" })
local screenshot_image = root():screenshot()
assert_image_ref(screenshot_image, "root screenshot")

return {
    image = screenshot_image,
}
