eguidev.configure({ timeout_ms = 10000, poll_interval_ms = 25 })

eguidev.fixture("hotki.permissions.none_granted")
local permissions = eguidev.wait_viewport({ name = "permissions" })

permissions:widget("permissions.description"):wait({ visible = true })
local accessibility = assert(permissions:widget("permissions.accessibility.status"):state())
local input_monitoring = assert(permissions:widget("permissions.input_monitoring.status"):state())

assert(accessibility.value_text == "Not granted", "accessibility should be missing")
assert(input_monitoring.value_text == "Not granted", "input monitoring should be missing")
assert(assert(permissions:widget("permissions.accessibility.granted"):state()).value == false)
assert(assert(permissions:widget("permissions.input_monitoring.granted"):state()).value == false)
assert(assert(permissions:widget("permissions.accessibility.open_settings"):state()).role == "button")
assert(assert(permissions:widget("permissions.input_monitoring.open_settings"):state()).role == "button")

permissions:widget("permissions.accessibility.open_settings"):click()
permissions:widget("permissions.accessibility.open_intent"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)

eguidev.fixture("hotki.permissions.all_granted")
permissions = eguidev.wait_viewport({ name = "permissions" })
permissions:widget("permissions.accessibility.granted"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
permissions:widget("permissions.input_monitoring.granted"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)

eguidev.fixture("hotki.permissions.mixed")
permissions = eguidev.wait_viewport({ name = "permissions" })
permissions:widget("permissions.accessibility.granted"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
permissions:widget("permissions.input_monitoring.granted"):wait(function(widget)
    return widget ~= nil and widget.value == false
end)

return {
    accessibility = accessibility.value_text,
    input_monitoring = input_monitoring.value_text,
}
