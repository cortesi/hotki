eguidev.configure({ timeout_ms = 10000, poll_interval_ms = 25 })

eguidev.fixture("hotki.permissions.hidden_missing")
assert(#eguidev.viewports({ name = "permissions" }) == 0, "permissions helper should remain hidden")

local missing = eguidev.diagnostic("hotki.state") :: any
assert(missing.runtime.permissions.accessibility == "denied", "accessibility should be denied")
assert(
    missing.runtime.permissions.input_monitoring == "denied",
    "input monitoring should be denied"
)
assert(missing.runtime.phase == "waiting_permissions", "runtime should wait for permissions")
assert(missing.runtime.retry == "available", "permission recovery should be available")

eguidev.fixture("hotki.permissions.hidden_recovered")
assert(#eguidev.viewports({ name = "permissions" }) == 0, "permissions helper should remain hidden")

local recovered = eguidev.diagnostic("hotki.state") :: any
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
