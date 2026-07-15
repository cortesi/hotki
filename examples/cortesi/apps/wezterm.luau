local a = hotki.actions

local function relay_menu(menu, prefix: string, stay: boolean)
    local relay = a.relay_with(prefix)
    local target: MenuBuilder = if stay then menu:with({ stay = true }) else menu
    target:bind("k", "up", relay("k"))
    target:bind("j", "down", relay("j"))
    target:bind("h", "left", relay("h"))
    target:bind("l", "right", relay("l"))
end

return function(menu: MenuBuilder, _ctx: ModeContext)
    menu:submenu("shift+cmd+j", "wezterm", function(app_menu)
        app_menu:submenu("r", "resize pane", function(resize)
            relay_menu(resize, "ctrl+cmd+shift+", true)
        end)
        app_menu:submenu("s", "split pane", function(split)
            local relay = a.relay_with("ctrl+shift+cmd")
            split:bind("h", "left", relay("a"))
            split:bind("l", "right", relay("s"))
            split:bind("k", "up", relay("d"))
            split:bind("j", "down", relay("f"))
        end)
        app_menu:bind("t", "toggle tabs", a.relay_with("ctrl+shift+cmd")("t"))
    end)
end
