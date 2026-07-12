local a = hotki.actions

local function grid(cols: number, rows: number, x: number, y: number, w: number, h: number): Action
    return a.shell(string.format(
        "yabai -m window --grid %d:%d:%d:%d:%d:%d",
        rows,
        cols,
        x,
        y,
        w,
        h
    ))
end

local function cell(cols: number, rows: number, x: number, y: number): Action
    return grid(cols, rows, x, y, 1, 1)
end

local function focus_space(space: number): Action
    return a.shell(string.format("yabai -m space --focus %d", space))
end

local function move_to_space(space: number): Action
    return a.shell(string.format("yabai -m window --space %d", space))
end

return table.freeze({
    grid = grid,
    cell = cell,
    focus_space = focus_space,
    move_to_space = move_to_space,
})
