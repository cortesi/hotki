//! Trusted Luau prelude for the public `action` table.

/// Trusted source installed before the config VM is sealed for untrusted code.
pub(super) const ACTION_PRELUDE: &[u8] = br#"
local action_table = {}

local function expect_action(value, name)
    if type(value) ~= "function" then
        error(name .. " expects an action function", 3)
    end
    return value
end

local function expect_string(value, name)
    if type(value) ~= "string" then
        error(name .. " must be a string", 3)
    end
    return value
end

local function expect_number(value, name)
    if type(value) ~= "number" then
        error(name .. " must be a number", 3)
    end
    return value
end

local function expect_table(value, name)
    if type(value) ~= "table" then
        error(name .. " must be a table", 3)
    end
    return value
end

local function expect_options(value, name)
    if value ~= nil and type(value) ~= "table" then
        error(name .. " must be a table", 3)
    end
    return value
end

local function expect_toggle(value, name)
    if value ~= "on" and value ~= "off" and value ~= "toggle" then
        error(name .. " must be \"on\", \"off\", or \"toggle\"", 3)
    end
    return value
end

action_table.pop = function(ctx)
    ctx:pop()
end

action_table.exit = function(ctx)
    ctx:exit()
end

action_table.show_root = function(ctx)
    ctx:show_root()
end

action_table.hide_hud = function(ctx)
    ctx:hide_hud()
end

action_table.reload_config = function(ctx)
    ctx:reload_config()
end

action_table.clear_notifications = function(ctx)
    ctx:clear_notifications()
end

action_table.shell = function(cmd, opts)
    cmd = expect_string(cmd, "action.shell command")
    opts = expect_options(opts, "action.shell options")
    return function(ctx)
        ctx:shell(cmd, opts)
    end
end

action_table.open = function(target)
    target = expect_string(target, "action.open target")
    return function(ctx)
        ctx:open(target)
    end
end

action_table.relay = function(spec)
    spec = expect_string(spec, "action.relay spec")
    return function(ctx)
        ctx:relay(spec)
    end
end

action_table.show_details = function(toggle)
    toggle = expect_toggle(toggle, "action.show_details toggle")
    return function(ctx)
        ctx:show_details(toggle)
    end
end

action_table.set_volume = function(level)
    level = expect_number(level, "action.set_volume level")
    return function(ctx)
        ctx:set_volume(level)
    end
end

action_table.change_volume = function(delta)
    delta = expect_number(delta, "action.change_volume delta")
    return function(ctx)
        ctx:change_volume(delta)
    end
end

action_table.mute = function(toggle)
    toggle = expect_toggle(toggle, "action.mute toggle")
    return function(ctx)
        ctx:mute(toggle)
    end
end

action_table.selector = function(spec)
    spec = expect_table(spec, "action.selector spec")
    return function(ctx)
        ctx:select(spec)
    end
end

action = table.freeze(action_table)
"#;
