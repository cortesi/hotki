configure({ timeout_ms = 10000, poll_interval_ms = 25 })

fixture("hotki.tray.about")
expect("app.ready", { viewport = "root", value = true })
expect("app.about.opened", { viewport = "root", value = true })
wait_for_frames(2)

return {
    about_invoked = true,
}
