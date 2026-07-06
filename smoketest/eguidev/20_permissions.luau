configure({ timeout_ms = 10000, poll_interval_ms = 25 })

fixture("hotki.permissions.none_granted")
local permissions = viewport({ name = "permissions" })

permissions:wait_for_widget_visible("permissions.description")
local accessibility = permissions:widget_get("permissions.accessibility.status"):state()
local input_monitoring = permissions:widget_get("permissions.input_monitoring.status"):state()

assert(accessibility.value_text == "Not granted", "accessibility should be missing")
assert(input_monitoring.value_text == "Not granted", "input monitoring should be missing")
assert(permissions:widget_get("permissions.accessibility.granted"):state().value_text == "false")
assert(permissions:widget_get("permissions.input_monitoring.granted"):state().value_text == "false")
assert(permissions:widget_get("permissions.accessibility.open_settings"):state().role == "button")
assert(permissions:widget_get("permissions.input_monitoring.open_settings"):state().role == "button")

permissions:widget_get("permissions.accessibility.open_settings"):click()
permissions:wait_for_widget("permissions.accessibility.open_intent", function(widget)
    return widget.value_text == "true"
end)

fixture("hotki.permissions.all_granted")
permissions = viewport({ name = "permissions" })
permissions:wait_for_widget("permissions.accessibility.granted", function(widget)
    return widget.value_text == "true"
end)
permissions:wait_for_widget("permissions.input_monitoring.granted", function(widget)
    return widget.value_text == "true"
end)

fixture("hotki.permissions.mixed")
permissions = viewport({ name = "permissions" })
permissions:wait_for_widget("permissions.accessibility.granted", function(widget)
    return widget.value_text == "true"
end)
permissions:wait_for_widget("permissions.input_monitoring.granted", function(widget)
    return widget.value_text == "false"
end)

return {
    accessibility = accessibility.value_text,
    input_monitoring = input_monitoring.value_text,
}
