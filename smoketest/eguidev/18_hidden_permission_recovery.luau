configure({ timeout_ms = 10000, poll_interval_ms = 25 })

fixture("hotki.permissions.hidden_missing")
assert(viewport({ name = "permissions" }) == nil, "permissions helper should remain hidden")

local missing = diagnostic("hotki.state") :: any
assert(missing.runtime.permissions.accessibility == "denied", "accessibility should be denied")
assert(
    missing.runtime.permissions.input_monitoring == "denied",
    "input monitoring should be denied"
)
assert(missing.runtime.phase == "waiting_permissions", "runtime should wait for permissions")
assert(missing.runtime.retry == "available", "permission recovery should be available")

fixture("hotki.permissions.hidden_recovered")
assert(viewport({ name = "permissions" }) == nil, "permissions helper should remain hidden")

local recovered = diagnostic("hotki.state") :: any
assert(
    recovered.runtime.permissions.accessibility == "granted",
    "accessibility should recover"
)
assert(
    recovered.runtime.permissions.input_monitoring == "granted",
    "input monitoring should recover"
)
assert(recovered.runtime.phase == "ready", "runtime should recover without opening the helper")
assert(recovered.runtime.retry == "idle", "recovered runtime should not retain retry state")

return {
    before = missing.runtime.phase,
    after = recovered.runtime.phase,
    retry = recovered.runtime.retry,
}
