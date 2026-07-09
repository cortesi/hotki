-- A rough facsimile of cortesi's yabai + wezterm + spotify config.
--
-- This config assumes:
-- - `yabai` is installed and running
-- - `spotify` is available on PATH (or adjust commands)
-- - `/opt/homebrew/bin/wezterm` exists (or adjust)

local DEFAULT_REPEAT: RepeatOptions = {}
local GLOBAL_HIDDEN: BindingOptions = { global = true, hidden = true }
local STAY: BindingOptions = { stay = true }
local CAPTURE: SubmenuOptions = { capture = true }

local function yabai_grid(
    cols: number,
    rows: number,
    x: number,
    y: number,
    w: number,
    h: number
): Action
    return function(actx: ActionContext)
        actx:shell(string.format(
            "yabai -m window --grid %d:%d:%d:%d:%d:%d",
            rows,
            cols,
            x,
            y,
            w,
            h
        ))
    end
end

local function yabai_cell(cols: number, rows: number, x: number, y: number): Action
    return yabai_grid(cols, rows, x, y, 1, 1)
end

local function yabai_cell_move(cols: number, rows: number, dx: number, dy: number): Action
    return function(actx: ActionContext)
        actx:shell(string.format([[
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
end

local function bind_space_switches(menu: MenuBuilder)
    for i = 1, 9 do
        local space = i
        local label = tostring(space)
        menu:bind("cmd+" .. label, label, function(actx)
            actx:shell(string.format("yabai -m space --focus %d", space))
        end, GLOBAL_HIDDEN)
    end
    menu:bind("cmd+0", "10", function(actx)
        actx:shell("yabai -m space --focus 10")
    end, GLOBAL_HIDDEN)
end

local function bind_move_to_space(menu: MenuBuilder)
    for i = 1, 9 do
        local space = i
        local label = tostring(space)
        menu:bind(label, label, function(actx)
            actx:shell(string.format("yabai -m window --space %d", space))
        end)
    end
end

hotki.root(function(menu, ctx)
    menu:bind("shift+cmd+p", "Run Application", function(actx)
        actx:select({
            title = "Run Application",
            placeholder = "Search apps...",
            items = hotki.applications,
            on_select = function(
                select_ctx: ActionContext,
                item: SelectorItem<ApplicationInfo>,
                _query: string
            )
                select_ctx:open(item.data.path)
            end,
        })
    end)

    bind_space_switches(menu)

    menu:bind("cmd+return", "wezterm window", function(actx)
        actx:shell("/opt/homebrew/bin/wezterm cli spawn --new-window")
    end, GLOBAL_HIDDEN)
    menu:bind("shift+cmd+y", "y scratchpad", function(actx)
        actx:shell("yabai -m window --toggle web")
    end, GLOBAL_HIDDEN)
    menu:bind("shift+cmd+u", "u scratchpad", function(actx)
        actx:shell("yabai -m window --toggle web2")
    end, GLOBAL_HIDDEN)

    if ctx.hud then
        menu:bind("esc", "Exit", function(actx)
            actx:exit()
        end, GLOBAL_HIDDEN)
        menu:bind("left", "Pop", function(actx)
            actx:pop()
        end, GLOBAL_HIDDEN)
    end

    menu:submenu("shift+cmd+m", "music", function(music, _music_ctx)
        music:bind("k", "vol up", function(actx)
            actx:until_keyup(function(repeat_ctx)
                repeat_ctx:change_volume(5)
            end, DEFAULT_REPEAT)
        end, STAY)
        music:bind("j", "vol down", function(actx)
            actx:until_keyup(function(repeat_ctx)
                repeat_ctx:change_volume(-5)
            end, DEFAULT_REPEAT)
        end, STAY)
        music:bind("l", "next >>", function(actx)
            actx:shell("spotify next")
        end)
        music:bind("h", "<< prev", function(actx)
            actx:shell("spotify prev")
        end)
        music:bind("p", "play/pause", function(actx)
            actx:shell("spotify pause")
        end)
        music:bind("m", "mute", function(actx)
            actx:mute("toggle")
        end)
        music:bind("shift+cmd+m", "exit", function(actx)
            actx:exit()
        end, GLOBAL_HIDDEN)
    end, CAPTURE)

    menu:submenu("shift+cmd+space", "activate", function(root, _root_ctx)
        root:submenu("h", "hotki", function(sub, _subctx)
            sub:bind("c", "Clear Notifications", function(actx)
                actx:clear_notifications()
            end)
            sub:bind("d", "Details", function(actx)
                actx:show_details("toggle")
            end)
            sub:bind("r", "Reload Config", function(actx)
                actx:reload_config()
            end)
            sub:bind("o", "Show Root Keys", function(actx)
                actx:show_root()
            end)
        end)

        root:submenu("g", "go", function(go, _go_ctx)
            go:bind("u", "u scratchpad", function(actx)
                actx:shell("yabai -m window --toggle web2")
            end)
            go:bind("y", "y scratchpad", function(actx)
                actx:shell("yabai -m window --toggle web")
            end)
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

            win:bind("f", "fullscreen", function(actx)
                actx:shell("yabai -m window --toggle zoom-fullscreen")
            end)
            win:bind("n", "native fullscreen", function(actx)
                actx:shell("yabai -m window --toggle native-fullscreen")
            end)
            win:bind("u", "set u scratchpad", function(actx)
                actx:shell("yabai -m window --scratchpad web2")
            end)
            win:bind("y", "set y scratchpad", function(actx)
                actx:shell("yabai -m window --scratchpad web")
            end)
        end)

        root:bind("shift+cmd+space", "exit", function(actx)
            actx:exit()
        end, GLOBAL_HIDDEN)
    end, CAPTURE)

    if ctx:app_matches("WezTerm") then
        menu:submenu("shift+cmd+j", "wezterm", function(app_menu, _app_ctx)
            app_menu:submenu("r", "resize pane", function(sub, _subctx)
                sub:bind("k", "up", function(actx)
                    actx:relay("ctrl+cmd+shift+k")
                end, STAY)
                sub:bind("j", "down", function(actx)
                    actx:relay("ctrl+cmd+shift+j")
                end, STAY)
                sub:bind("h", "left", function(actx)
                    actx:relay("ctrl+cmd+shift+h")
                end, STAY)
                sub:bind("l", "right", function(actx)
                    actx:relay("ctrl+cmd+shift+l")
                end, STAY)
            end)

            app_menu:submenu("s", "split pane", function(sub, _subctx)
                sub:bind("h", "left", function(actx)
                    actx:relay("ctrl+shift+cmd+a")
                end)
                sub:bind("l", "right", function(actx)
                    actx:relay("ctrl+shift+cmd+s")
                end)
                sub:bind("k", "up", function(actx)
                    actx:relay("ctrl+shift+cmd+d")
                end)
                sub:bind("j", "down", function(actx)
                    actx:relay("ctrl+shift+cmd+f")
                end)
            end)

            app_menu:bind("t", "toggle tabs", function(actx)
                actx:relay("ctrl+shift+cmd+t")
            end)
        end)
    elseif ctx:app_matches("Brave") then
        menu:submenu("shift+cmd+j", "brave", function(app_menu, _app_ctx)
            app_menu:submenu("b", "bookmarks", function(sub, _subctx)
                sub:bind("b", "toggle bar", function(actx)
                    actx:relay("shift+cmd+b")
                end)
            end)
        end)
    elseif ctx:app_matches("Obsidian") then
        menu:submenu("shift+cmd+j", "obsidian", function(app_menu, _app_ctx)
            app_menu:submenu("c", "content", function(content, _content_ctx)
                content:submenu("i", "insert", function(insert, _insert_ctx)
                    insert:bind("a", "attachment", function(actx)
                        actx:relay("ctrl+shift+cmd+6")
                    end)
                    insert:bind("c", "code", function(actx)
                        actx:relay("ctrl+shift+cmd+7")
                    end)
                    insert:bind("f", "footnote", function(actx)
                        actx:relay("ctrl+shift+cmd+8")
                    end)
                    insert:bind("l", "link", function(actx)
                        actx:relay("ctrl+shift+cmd+9")
                    end)
                end)

                content:submenu("t", "table", function(tbl, _table_ctx)
                    tbl:bind("n", "new", function(actx)
                        actx:relay("ctrl+shift+cmd+y")
                    end, STAY)
                    tbl:submenu("i", "insert", function(insert, _insert_ctx)
                        insert:bind("h", "column left", function(actx)
                            actx:relay("ctrl+shift+cmd+u")
                        end, STAY)
                        insert:bind("l", "column right", function(actx)
                            actx:relay("ctrl+shift+cmd+i")
                        end, STAY)
                        insert:bind("j", "column above", function(actx)
                            actx:relay("ctrl+shift+cmd+o")
                        end, STAY)
                        insert:bind("k", "column below", function(actx)
                            actx:relay("ctrl+shift+cmd+p")
                        end, STAY)
                    end)
                    tbl:submenu("d", "delete", function(delete_menu, _delete_ctx)
                        delete_menu:bind("r", "row", function(actx)
                            actx:relay("ctrl+shift+cmd+m")
                        end, STAY)
                        delete_menu:bind("c", "column", function(actx)
                            actx:relay("ctrl+shift+cmd+n")
                        end, STAY)
                    end)
                end)
            end)

            app_menu:submenu("f", "file", function(file_menu, _file_ctx)
                file_menu:bind("f", "show in finder", function(actx)
                    actx:relay("ctrl+shift+cmd+4")
                end)
                file_menu:bind("n", "rename", function(actx)
                    actx:relay("ctrl+shift+cmd+5")
                end)
            end)

            app_menu:submenu("s", "split", function(split, _split_ctx)
                split:bind("l", "right", function(actx)
                    actx:relay("ctrl+shift+cmd+2")
                end)
                split:bind("j", "down", function(actx)
                    actx:relay("ctrl+shift+cmd+1")
                end)
            end)

            app_menu:submenu("u", "utils", function(utils, _utils_ctx)
                utils:bind("s", "shell here", function(actx)
                    actx:relay("ctrl+shift+cmd+s")
                end)
            end)

            app_menu:submenu("w", "window", function(window, _window_ctx)
                window:bind("h", "sidebar left", function(actx)
                    actx:relay("ctrl+shift+cmd+h")
                end)
                window:bind("l", "sidebar right", function(actx)
                    actx:relay("ctrl+shift+cmd+l")
                end)
                window:bind("s", "settings", function(actx)
                    actx:relay("ctrl+shift+cmd+3")
                end)
            end)
        end)
    end
end)
