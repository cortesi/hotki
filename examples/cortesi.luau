-- A rough facsimile of cortesi's yabai + wezterm + spotify config.
--
-- This config assumes:
-- - `yabai` is installed and running
-- - `spotify` is available on PATH (or adjust commands)
-- - `/opt/homebrew/bin/wezterm` exists (or adjust)

themes:use("default")

local DEFAULT_REPEAT: RepeatOptions = {}
local GLOBAL_HIDDEN: BindingOptions = { global = true, hidden = true }
local STAY: BindingOptions = { stay = true }
local REPEAT: BindingOptions = { ["repeat"] = DEFAULT_REPEAT }
local CAPTURE: SubmenuOptions = { capture = true }

local function yabai_grid(cols: number, rows: number, x: number, y: number, w: number, h: number): Action
    return action.shell(string.format("yabai -m window --grid %d:%d:%d:%d:%d:%d", rows, cols, x, y, w, h))
end

local function yabai_cell(cols: number, rows: number, x: number, y: number): Action
    return yabai_grid(cols, rows, x, y, 1, 1)
end

local function yabai_cell_move(cols: number, rows: number, dx: number, dy: number): Action
    return action.shell(string.format([[
osascript -l JavaScript \
  -e 'const app = Application.currentApplication(); app.includeStandardAdditions = true;' \
  -e 'const cols = %d; const rows = %d; const dx = %d; const dy = %d;' \
  -e 'function query(args) { return JSON.parse(app.doShellScript("yabai -m query " + args)); }' \
  -e 'const win = query("--windows --window"); const disp = query("--displays --display");' \
  -e 'const wf = win.frame; const df = disp.frame;' \
  -e 'const wx = wf.x, wy = wf.y, ww = wf.w, wh = wf.h;' \
  -e 'const ox = df.x, oy = df.y, dw = df.w, dh = df.h;' \
  -e 'const cellW = dw / cols; const cellH = dh / rows;' \
  -e 'let x = Math.round((wx - ox) / cellW); let y = Math.round((wy - oy) / cellH);' \
  -e 'let w = Math.max(1, Math.round(ww / cellW)); let h = Math.max(1, Math.round(wh / cellH));' \
  -e 'x = Math.min(Math.max(x + dx, 0), cols - w); y = Math.min(Math.max(y + dy, 0), rows - h);' \
  -e 'const spec = rows + ":" + cols + ":" + x + ":" + y + ":" + w + ":" + h; app.doShellScript("yabai -m window --grid " + spec);'
]], cols, rows, dx, dy))
end

local function bind_space_switches(menu: MenuBuilder)
    for i = 1, 9 do
        local label = tostring(i)
        menu:bind("cmd+" .. label, label, action.shell(string.format("yabai -m space --focus %d", i)), GLOBAL_HIDDEN)
    end
    menu:bind("cmd+0", "10", action.shell("yabai -m space --focus 10"), GLOBAL_HIDDEN)
end

local function bind_move_to_space(menu: MenuBuilder)
    for i = 1, 9 do
        local label = tostring(i)
        menu:bind(label, label, action.shell(string.format("yabai -m window --space %d", i)))
    end
end

hotki.root(function(menu, ctx)
    menu:bind("shift+cmd+p", "Run Application", action.selector({
        title = "Run Application",
        placeholder = "Search apps...",
        items = hotki.applications,
        on_select = function(actx: ActionContext, item: SelectorItem<ApplicationInfo>, _query: string)
            actx:exec(action.open(item.data.path))
        end,
    }))

    bind_space_switches(menu)

    menu:bind(
        "cmd+return",
        "wezterm window",
        action.shell("/opt/homebrew/bin/wezterm cli spawn --new-window"),
        GLOBAL_HIDDEN
    )
    menu:bind("shift+cmd+y", "y scratchpad", action.shell("yabai -m window --toggle web"), GLOBAL_HIDDEN)
    menu:bind("shift+cmd+u", "u scratchpad", action.shell("yabai -m window --toggle web2"), GLOBAL_HIDDEN)

    if ctx.hud then
        menu:bind("esc", "Exit", action.exit, GLOBAL_HIDDEN)
        menu:bind("left", "Pop", action.pop, GLOBAL_HIDDEN)
    end

    menu:submenu("shift+cmd+m", "music", function(music, _music_ctx)
        music:bind("k", "vol up", action.change_volume(5), REPEAT)
        music:bind("j", "vol down", action.change_volume(-5), REPEAT)
        music:bind("l", "next >>", action.shell("spotify next"))
        music:bind("h", "<< prev", action.shell("spotify prev"))
        music:bind("p", "play/pause", action.shell("spotify pause"))
        music:bind("m", "mute", action.mute("toggle"))
        music:bind("shift+cmd+m", "exit", action.exit, GLOBAL_HIDDEN)
    end, CAPTURE)

    menu:submenu("shift+cmd+space", "activate", function(root, _root_ctx)
        root:submenu("h", "hotki", function(sub, _subctx)
            sub:bind("c", "Clear Notifications", action.clear_notifications)
            sub:bind("d", "Details", action.show_details("toggle"))
            sub:bind("r", "Reload Config", action.reload_config)
            sub:bind("o", "Show Root Keys", action.show_root)
            sub:bind("t", "Theme", action.theme_next, STAY)
        end)

        root:submenu("g", "go", function(go, _go_ctx)
            go:bind("u", "u scratchpad", action.shell("yabai -m window --toggle web2"))
            go:bind("y", "y scratchpad", action.shell("yabai -m window --toggle web"))
        end)

        root:submenu("w", "window", function(win, _win_ctx)
            win:submenu("2", "2 Columns", function(cols, _cols_ctx)
                cols:bind("h", "left", yabai_cell(2, 1, 0, 0))
                cols:bind("l", "right", yabai_cell(2, 1, 1, 0))
            end)

            win:submenu("3", "3 Columns", function(cols, _cols_ctx)
                cols:bind("1", "Column 1", yabai_cell(3, 1, 0, 0))
                cols:bind("2", "Column 2", yabai_cell(3, 1, 1, 0))
                cols:bind("3", "Column 3", yabai_cell(3, 1, 2, 0))
                cols:bind("l", "left", yabai_cell_move(3, 1, 1, 0), STAY)
                cols:bind("h", "right", yabai_cell_move(3, 1, -1, 0), STAY)
            end)

            win:submenu("g", "3x3 Grid", function(grid, _grid_ctx)
                grid:bind("l", "left", yabai_cell_move(3, 3, 1, 0), STAY)
                grid:bind("h", "right", yabai_cell_move(3, 3, -1, 0), STAY)
                grid:bind("k", "up", yabai_cell_move(3, 3, 0, -1), STAY)
                grid:bind("j", "down", yabai_cell_move(3, 3, 0, 1), STAY)
            end)

            win:submenu("s", "move to space", function(spaces, _spaces_ctx)
                bind_move_to_space(spaces)
            end)

            win:bind("f", "fullscreen", action.shell("yabai -m window --toggle zoom-fullscreen"))
            win:bind("n", "native fullscreen", action.shell("yabai -m window --toggle native-fullscreen"))
            win:bind("u", "set u scratchpad", action.shell("yabai -m window --scratchpad web2"))
            win:bind("y", "set y scratchpad", action.shell("yabai -m window --scratchpad web"))
        end)

        root:bind("shift+cmd+space", "exit", action.exit, GLOBAL_HIDDEN)
    end, CAPTURE)

    if ctx:app_matches("WezTerm") then
        menu:submenu("shift+cmd+j", "wezterm", function(app_menu, _app_ctx)
            app_menu:submenu("r", "resize pane", function(sub, _subctx)
                sub:bind("k", "up", action.relay("ctrl+cmd+shift+k"), STAY)
                sub:bind("j", "down", action.relay("ctrl+cmd+shift+j"), STAY)
                sub:bind("h", "left", action.relay("ctrl+cmd+shift+h"), STAY)
                sub:bind("l", "right", action.relay("ctrl+cmd+shift+l"), STAY)
            end)

            app_menu:submenu("s", "split pane", function(sub, _subctx)
                sub:bind("h", "left", action.relay("ctrl+shift+cmd+a"))
                sub:bind("l", "right", action.relay("ctrl+shift+cmd+s"))
                sub:bind("k", "up", action.relay("ctrl+shift+cmd+d"))
                sub:bind("j", "down", action.relay("ctrl+shift+cmd+f"))
            end)

            app_menu:bind("t", "toggle tabs", action.relay("ctrl+shift+cmd+t"))
        end)
    elseif ctx:app_matches("Brave") then
        menu:submenu("shift+cmd+j", "brave", function(app_menu, _app_ctx)
            app_menu:submenu("b", "bookmarks", function(sub, _subctx)
                sub:bind("b", "toggle bar", action.relay("shift+cmd+b"))
            end)
        end)
    elseif ctx:app_matches("Obsidian") then
        menu:submenu("shift+cmd+j", "obsidian", function(app_menu, _app_ctx)
            app_menu:submenu("c", "content", function(content, _content_ctx)
                content:submenu("i", "insert", function(insert, _insert_ctx)
                    insert:bind("a", "attachment", action.relay("ctrl+shift+cmd+6"))
                    insert:bind("c", "code", action.relay("ctrl+shift+cmd+7"))
                    insert:bind("f", "footnote", action.relay("ctrl+shift+cmd+8"))
                    insert:bind("l", "link", action.relay("ctrl+shift+cmd+9"))
                end)

                content:submenu("t", "table", function(tbl, _table_ctx)
                    tbl:bind("n", "new", action.relay("ctrl+shift+cmd+y"), STAY)
                    tbl:submenu("i", "insert", function(insert, _insert_ctx)
                        insert:bind("h", "column left", action.relay("ctrl+shift+cmd+u"), STAY)
                        insert:bind("l", "column right", action.relay("ctrl+shift+cmd+i"), STAY)
                        insert:bind("j", "column above", action.relay("ctrl+shift+cmd+o"), STAY)
                        insert:bind("k", "column below", action.relay("ctrl+shift+cmd+p"), STAY)
                    end)
                    tbl:submenu("d", "delete", function(delete_menu, _delete_ctx)
                        delete_menu:bind("r", "row", action.relay("ctrl+shift+cmd+m"), STAY)
                        delete_menu:bind("c", "column", action.relay("ctrl+shift+cmd+n"), STAY)
                    end)
                end)
            end)

            app_menu:submenu("f", "file", function(file_menu, _file_ctx)
                file_menu:bind("f", "show in finder", action.relay("ctrl+shift+cmd+4"))
                file_menu:bind("n", "rename", action.relay("ctrl+shift+cmd+5"))
            end)

            app_menu:submenu("s", "split", function(split, _split_ctx)
                split:bind("l", "right", action.relay("ctrl+shift+cmd+2"))
                split:bind("j", "down", action.relay("ctrl+shift+cmd+1"))
            end)

            app_menu:submenu("u", "utils", function(utils, _utils_ctx)
                utils:bind("s", "shell here", action.relay("ctrl+shift+cmd+s"))
            end)

            app_menu:submenu("w", "window", function(window, _window_ctx)
                window:bind("h", "sidebar left", action.relay("ctrl+shift+cmd+h"))
                window:bind("l", "sidebar right", action.relay("ctrl+shift+cmd+l"))
                window:bind("s", "settings", action.relay("ctrl+shift+cmd+3"))
            end)
        end)
    end
end)
