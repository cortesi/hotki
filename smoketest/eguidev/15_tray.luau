eguidev.configure({ timeout_ms = 10000, poll_interval_ms = 25 })

eguidev.fixture("hotki.tray.about")
assert(assert(eguidev.widget("app.ready"):state()).value == true)
assert(assert(eguidev.widget("app.about.opened"):state()).value == true)

return {
    about_invoked = true,
}
