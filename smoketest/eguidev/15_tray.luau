eguidev.configure({ timeout_ms = 10000, poll_interval_ms = 25 })

eguidev.fixture("hotki.basic.default")
eguidev.widget("app.about.opened"):wait({ value = false })

eguidev.fixture("hotki.tray.about")
eguidev.widget("app.ready"):expect({ value = true })
local about = eguidev.widget("app.about.opened"):expect({ value = true })

return {
    about_visible = assert(about).value,
}
