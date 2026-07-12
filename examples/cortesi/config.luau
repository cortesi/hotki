-- A compact facsimile of cortesi's yabai + wezterm + spotify config.

local a = hotki.actions
local yabai = require("./actions")
local music_commands = require("./data/music")
local wezterm = require("./apps/wezterm")
local obsidian = require("./apps/obsidian")

local GLOBAL = { global = true, hidden = true }
local STAY = { stay = true }
local CAPTURE = { capture = true }

local function bind_space_switches(menu)
    for space = 1, 9 do
        menu:bind("cmd+" .. tostring(space), tostring(space), yabai.focus_space(space), GLOBAL)
    end
    menu:bind("cmd+0", "10", yabai.focus_space(10), GLOBAL)
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
    end)
    menu:submenu("s", "move to space", bind_move_to_space)
    menu:bind("f", "fullscreen", a.shell("yabai -m window --toggle zoom-fullscreen"))
    menu:bind("n", "native fullscreen", a.shell("yabai -m window --toggle native-fullscreen"))
end

return function(menu, ctx)
    menu:bind("shift+cmd+p", "Run Application", a.select({
        placeholder = "Search apps...",
        items = hotki.applications,
        on_select = function(select_ctx, item)
            select_ctx:open(item.data.path)
        end,
    }))
    bind_space_switches(menu)

    menu:bind("cmd+return", "wezterm window", a.shell(
        "/opt/homebrew/bin/wezterm cli spawn --new-window"
    ), GLOBAL)
    menu:bind("shift+cmd+y", "y scratchpad", a.shell("yabai -m window --toggle web"), GLOBAL)
    menu:bind("shift+cmd+u", "u scratchpad", a.shell("yabai -m window --toggle web2"), GLOBAL)

    if ctx.hud then
        menu:bind("esc", "Exit", a.exit, GLOBAL)
        menu:bind("left", "Pop", a.pop, GLOBAL)
    end

    menu:submenu("shift+cmd+m", "music", function(music)
        music:bind("k", "vol up", a.hold(a.change_volume(5)), STAY)
        music:bind("j", "vol down", a.hold(a.change_volume(-5)), STAY)
        for _, command in music_commands do
            music:bind(command.chord, command.desc, a.shell(command.command))
        end
        music:bind("m", "mute", a.mute("toggle"))
        music:bind("shift+cmd+m", "exit", a.exit, GLOBAL)
    end, CAPTURE)

    menu:submenu("shift+cmd+space", "activate", function(root)
        root:submenu("h", "hotki", function(hotki_menu)
            hotki_menu:bind("c", "Clear Notifications", a.clear_notifications)
            hotki_menu:bind("d", "Details", a.show_details("toggle"))
            hotki_menu:bind("r", "Reload Config", a.reload_config)
            hotki_menu:bind("o", "Show Root Keys", a.show_root)
        end)
        root:submenu("w", "window", bind_window_menu)
        root:bind("shift+cmd+space", "exit", a.exit, GLOBAL)
    end, CAPTURE)

    if ctx:app_matches("WezTerm") then
        wezterm(menu)
    elseif ctx:app_matches("Brave") then
        menu:bind("shift+cmd+b", "bookmarks", a.relay("shift+cmd+b"))
    elseif ctx:app_matches("Obsidian") then
        obsidian(menu)
    end
end
