local a = hotki.actions
local STAY = { stay = true }

local function relay_menu(menu, prefix: string, stay: boolean)
    local options = if stay then STAY else nil
    menu:bind("k", "up", a.relay(prefix .. "k"), options)
    menu:bind("j", "down", a.relay(prefix .. "j"), options)
    menu:bind("h", "left", a.relay(prefix .. "h"), options)
    menu:bind("l", "right", a.relay(prefix .. "l"), options)
end

return function(menu: MenuBuilder)
    menu:submenu("shift+cmd+j", "wezterm", function(app_menu)
        app_menu:submenu("r", "resize pane", function(resize)
            relay_menu(resize, "ctrl+cmd+shift+", true)
        end)
        app_menu:submenu("s", "split pane", function(split)
            split:bind("h", "left", a.relay("ctrl+shift+cmd+a"))
            split:bind("l", "right", a.relay("ctrl+shift+cmd+s"))
            split:bind("k", "up", a.relay("ctrl+shift+cmd+d"))
            split:bind("j", "down", a.relay("ctrl+shift+cmd+f"))
        end)
        app_menu:bind("t", "toggle tabs", a.relay("ctrl+shift+cmd+t"))
    end)
end
