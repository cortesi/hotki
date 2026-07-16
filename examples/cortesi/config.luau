-- A compact facsimile of cortesi's yabai + YouTube Music config.

local a = hotki.actions
local r = hotki.renderers
local yabai = require("./actions")
local obsidian = require("./apps/obsidian")
local youtube_music = a.relay_to_app("YouTube Music")

local function youtube_music_seek(spec: string)
    local seek = youtube_music(spec)
    return function(ctx)
        for _ = 1, 10 do
            seek(ctx)
        end
    end
end

local function bind_space_switches(menu)
    for space = 1, 9 do
        menu:bind("cmd+" .. tostring(space), tostring(space), yabai.focus_space(space))
    end
    menu:bind("cmd+0", "10", yabai.focus_space(10))
end

local function bind_move_to_space(menu)
    for space = 1, 9 do
        menu:bind(tostring(space), tostring(space), yabai.move_to_space(space))
    end
end

local function bind_window_menu(menu)
    menu:submenu("2", "2 Columns", function(cols)
        cols:bind("h", "left", yabai.cell(2, 1, 0, 0))
        cols:bind("l", "right", yabai.cell(2, 1, 1, 0))
    end)
    menu:submenu("3", "3 Columns", function(cols)
        for column = 1, 3 do
            cols:bind(tostring(column), "Column " .. tostring(column), yabai.cell(
                3,
                1,
                column - 1,
                0
            ))
        end
        local stay = cols:with({ stay = true })
        stay:bind("l", "left", yabai.move_cell(3, 1, 1, 0))
        stay:bind("h", "right", yabai.move_cell(3, 1, -1, 0))
    end)
    menu:submenu("g", "3x3 Grid", function(grid)
        local stay = grid:with({ stay = true })
        stay:bind("l", "left", yabai.move_cell(3, 3, 1, 0))
        stay:bind("h", "right", yabai.move_cell(3, 3, -1, 0))
        stay:bind("k", "up", yabai.move_cell(3, 3, 0, -1))
        stay:bind("j", "down", yabai.move_cell(3, 3, 0, 1))
    end)
    menu:submenu("s", "move to space", bind_move_to_space)
    menu:bind("f", "fullscreen", yabai.toggle_window_state("zoom-fullscreen"))
    menu:bind("n", "native fullscreen", yabai.toggle_window_state("native-fullscreen"))
    menu:bind("u", "set u scratchpad", yabai.set_scratchpad("web2"))
    menu:bind("y", "set y scratchpad", yabai.set_scratchpad("web"))
end

local function base(menu: MenuBuilder, ctx: ModeContext)
    local global = menu:with({ global = true, hidden = true })

    menu:bind("shift+cmd+p", "Run Application", a.launch_application())
    bind_space_switches(global)

    global:bind("shift+cmd+y", "y scratchpad", yabai.toggle_scratchpad("web"))
    global:bind("shift+cmd+u", "u scratchpad", yabai.toggle_scratchpad("web2"))

    if ctx.hud then
        global:bind("esc", "Exit", a.exit)
        global:bind("left", "Pop", a.pop)
    end

    menu:submenu("shift+cmd+m", "music", function(music)
        local music_stay = music:with({ stay = true })
        music_stay:bind("k", "vol up", a.hold(a.change_volume(5)))
        music_stay:bind("j", "vol down", a.hold(a.change_volume(-5)))
        music:bind("l", "next >>", youtube_music("j"))
        music:bind("h", "<< prev", youtube_music("k"))
        music_stay:bind("shift+l", "forward 10s", youtube_music_seek("shift+l"))
        music_stay:bind("shift+h", "back 10s", youtube_music_seek("shift+h"))
        music:bind("p", "play/pause", youtube_music("space"))
        music:bind("=", "like", youtube_music("shift+="))
        music:bind("-", "dislike", youtube_music("shift+-"))
        music:bind("m", "mute", a.mute("toggle"))
        music:bind("shift+cmd+m", "exit", a.exit, { global = true, hidden = true })
    end, { capture = true })

    menu:submenu("shift+cmd+space", "activate", function(root)
        root:submenu("h", "hotki", function(hotki_menu)
            hotki_menu:bind("c", "Clear Notifications", a.clear_notifications)
            hotki_menu:bind("d", "Details", a.show_details("toggle"))
            hotki_menu:bind("r", "Reload Config", a.reload_config)
            hotki_menu:bind("o", "Show Root Keys", a.show_root)
        end)
        root:submenu("g", "go", function(go)
            go:bind("u", "u scratchpad", yabai.toggle_scratchpad("web2"))
            go:bind("y", "y scratchpad", yabai.toggle_scratchpad("web"))
        end)
        root:submenu("w", "window", bind_window_menu)
        root:bind("shift+cmd+space", "exit", a.exit, { global = true, hidden = true })
    end, { capture = true })
end

local function brave(menu: MenuBuilder, _ctx: ModeContext)
    menu:submenu("shift+cmd+j", "brave", function(app_menu)
        app_menu:submenu("b", "bookmarks", function(bookmarks)
            bookmarks:bind("b", "toggle bar", a.relay("shift+cmd+b"))
        end)
    end)
end

return r.combine(
    base,
    r.when_app_matches("Brave", brave),
    r.when_app("Obsidian", obsidian)
)
