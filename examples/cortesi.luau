themes:use("default")

local function yabai_grid(cols, rows, x, y, w, h)
    return action.shell(string.format("yabai -m window --grid %d:%d:%d:%d:%d:%d", rows, cols, x, y, w, h))
end

local function yabai_cell(cols, rows, x, y)
    return yabai_grid(cols, rows, x, y, 1, 1)
end

hotki.root(function(menu, ctx)
    if ctx.hud then
        menu:bind("esc", "Exit", action.exit, { hidden = true, global = true })
        menu:bind("left", "Pop", action.pop, { hidden = true, global = true })
    end

    menu:submenu("shift+cmd+m", "music", function(music, inner)
        music:bind("k", "vol up", action.change_volume(5), { stay = true })
        music:bind("j", "vol down", action.change_volume(-5), { stay = true })
        music:bind("l", "next >>", action.shell("spotify next"), { stay = true })
        music:bind("h", "<< prev", action.shell("spotify prev"), { stay = true })
        music:bind("p", "play/pause", action.shell("spotify pause"), { stay = true })
        music:bind("m", "mute", action.mute("toggle"), { stay = true })
    end, { capture = true })

    menu:submenu("shift+cmd+space", "activate", function(root, inner)
        root:submenu("h", "hotki", function(sub, subctx)
            sub:bind("c", "Clear Notifications", action.clear_notifications)
            sub:bind("d", "Details", action.show_details("toggle"))
            sub:bind("r", "Reload Config", action.reload_config)
            sub:bind("o", "Show Root Keys", action.show_root)
            sub:bind("t", "Theme", action.theme_next, { stay = true })
        end)

        root:submenu("w", "window", function(win, winctx)
            win:submenu("2", "2 Columns", function(cols, cols_ctx)
                cols:bind("h", "left", yabai_cell(2, 1, 0, 0))
                cols:bind("l", "right", yabai_cell(2, 1, 1, 0))
            end)
            win:bind("f", "fullscreen", action.shell("yabai -m window --toggle zoom-fullscreen"))
        end)
    end, { capture = true })
end)
