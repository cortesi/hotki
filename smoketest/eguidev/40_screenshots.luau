configure({ timeout_ms = 15000, poll_interval_ms = 25 })

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

local function assert_image_ref(image: any, label: string)
    assert(image.type == "image_ref", label .. " should return an image_ref")
end

fixture("hotki.details")
local details = viewport_with_widget("details.root")
details:wait_for_widget_visible("details.root")

fixture("hotki.permissions.mixed")
local permissions = viewport_with_widget("permissions.root")
permissions:wait_for_widget_visible("permissions.root")

fixture("hotki.hud")
local hud = viewport_with_widget("hud.panel")
hud:wait_for_widget_visible("hud.panel")

fixture("hotki.selector")
local selector = viewport_with_widget("selector.panel")
selector:wait_for_widget_visible("selector.panel")

fixture("hotki.notifications")
local notification = nil
for _, viewport in ipairs(viewports()) do
    for _, widget in ipairs(viewport:widget_list({ id_prefix = "notification.live.", include_invisible = true })) do
        if string.sub(widget.id, -6) == ".title" and widget:state().value_text == "Eguidev" then
            notification = viewport
        end
    end
end
assert(notification ~= nil, "notification fixture should expose a live notification viewport")
local screenshot_image = root():screenshot()
assert_image_ref(screenshot_image, "root screenshot")

return {
    image = screenshot_image,
}
