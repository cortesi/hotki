eguidev.configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function wait_for_notification(title: string, text: string)
    local main = eguidev.wait_viewport({ name = "main" })
    eguidev.widget("main.activity.0.title"):wait(function(widget)
        return widget ~= nil and widget.value_text == title
    end)
    eguidev.widget("main.activity.0.text"):wait(function(widget)
        return widget ~= nil and string.find(widget.value_text, text, 1, true) ~= nil
    end)
    return main
end

eguidev.fixture("hotki.hud")
eguidev.fixture("hotki.hud.demo")
eguidev.fixture("hotki.selector")
eguidev.fixture("hotki.selector.query")
local selector = eguidev.wait_viewport({ name = "selector" })
eguidev.widget("selector.query"):wait(function(widget)
    return widget ~= nil and string.find(widget.value_text, "b", 1, true) ~= nil
end)
eguidev.widget("selector.item.0.label"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Beta"
end)
eguidev.fixture("hotki.selector.confirmed")
local confirmed = wait_for_notification("Selector", "Beta:b")
assert(assert(eguidev.widget("main.activity.0.mark"):state()).value_text == "info")

eguidev.fixture("hotki.hud")
eguidev.fixture("hotki.hud.demo")
eguidev.fixture("hotki.selector")
eguidev.fixture("hotki.selector.query")
selector = eguidev.wait_viewport({ name = "selector" })
eguidev.widget("selector.query"):wait(function(widget)
    return widget ~= nil and string.find(widget.value_text, "b", 1, true) ~= nil
end)
eguidev.fixture("hotki.selector.canceled")
local canceled = wait_for_notification("Selector", "cancel")
assert(assert(eguidev.widget("main.activity.0.mark"):state()).value_text == "warn")

return {
    confirmed = confirmed.id,
    canceled = canceled.id,
}
