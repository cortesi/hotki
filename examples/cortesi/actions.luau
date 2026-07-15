local a = hotki.actions
local YABAI = "/opt/homebrew/bin/yabai"

local function run_yabai(args: { string }): Action
    return a.exec({ program = YABAI, args = args })
end

local function grid(cols: number, rows: number, x: number, y: number, w: number, h: number): Action
    local spec = string.format("%d:%d:%d:%d:%d:%d", rows, cols, x, y, w, h)
    return run_yabai({ "-m", "window", "--grid", spec })
end

local function cell(cols: number, rows: number, x: number, y: number): Action
    return grid(cols, rows, x, y, 1, 1)
end

local function move_cell(cols: number, rows: number, dx: number, dy: number): Action
    return a.shell(string.format([[
osascript -l JavaScript \
  -e 'const app = Application.currentApplication(); app.includeStandardAdditions = true;' \
  -e 'const cols = %d; const rows = %d; const dx = %d; const dy = %d;' \
  -e 'function query(args) { return JSON.parse(app.doShellScript("/opt/homebrew/bin/yabai -m query " + args)); }' \
  -e 'const win = query("--windows --window"); const disp = query("--displays --display");' \
  -e 'const wf = win.frame; const df = disp.frame;' \
  -e 'const wx = wf.x, wy = wf.y, ww = wf.w, wh = wf.h;' \
  -e 'const ox = df.x, oy = df.y, dw = df.w, dh = df.h;' \
  -e 'const cellW = dw / cols; const cellH = dh / rows;' \
  -e 'let x = Math.round((wx - ox) / cellW); let y = Math.round((wy - oy) / cellH);' \
  -e 'let w = Math.max(1, Math.round(ww / cellW)); let h = Math.max(1, Math.round(wh / cellH));' \
  -e 'x = Math.min(Math.max(x + dx, 0), cols - w); y = Math.min(Math.max(y + dy, 0), rows - h);' \
  -e 'const spec = rows + ":" + cols + ":" + x + ":" + y + ":" + w + ":" + h; app.doShellScript("/opt/homebrew/bin/yabai -m window --grid " + spec);'
]], cols, rows, dx, dy))
end

local function focus_space(space: number): Action
    return run_yabai({ "-m", "space", "--focus", tostring(space) })
end

local function move_to_space(space: number): Action
    return run_yabai({ "-m", "window", "--space", tostring(space) })
end

local function toggle_window(option: string): Action
    return run_yabai({ "-m", "window", "--toggle", option })
end

local function set_scratchpad(name: string): Action
    return run_yabai({ "-m", "window", "--scratchpad", name })
end

return table.freeze({
    cell = cell,
    move_cell = move_cell,
    focus_space = focus_space,
    move_to_space = move_to_space,
    toggle_window = toggle_window,
    set_scratchpad = set_scratchpad,
})
