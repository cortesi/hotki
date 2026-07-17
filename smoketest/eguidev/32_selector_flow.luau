configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function wait_for_notification(title: string, text: string)
    local main = viewport({ name = "main" })
    main:wait_for_widget("main.activity.0.title", function(widget)
        return widget.value_text == title
    end)
    main:wait_for_widget("main.activity.0.text", function(widget)
        return string.find(widget.value_text, text, 1, true) ~= nil
    end)
    return main
end

fixture("hotki.hud")
fixture("hotki.hud.demo")
fixture("hotki.selector")
fixture("hotki.selector.query")
local selector = viewport({ name = "selector" })
selector:wait_for_widget("selector.query", function(widget)
    return string.find(widget.value_text, "b", 1, true) ~= nil
end)
selector:wait_for_widget("selector.item.0.label", function(widget)
    return widget.value_text == "Beta"
end)
fixture("hotki.selector.confirmed")
local confirmed = wait_for_notification("Selector", "Beta:b")
assert(confirmed:widget_get("main.activity.0.mark"):state().value_text == "info")

fixture("hotki.hud")
fixture("hotki.hud.demo")
fixture("hotki.selector")
fixture("hotki.selector.query")
selector = viewport({ name = "selector" })
selector:wait_for_widget("selector.query", function(widget)
    return string.find(widget.value_text, "b", 1, true) ~= nil
end)
fixture("hotki.selector.canceled")
local canceled = wait_for_notification("Selector", "cancel")
assert(canceled:widget_get("main.activity.0.mark"):state().value_text == "warn")

return {
    confirmed = confirmed.id,
    canceled = canceled.id,
}
