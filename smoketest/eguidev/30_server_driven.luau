eguidev.configure({ timeout_ms = 15000, poll_interval_ms = 25 })

local function require_viewport(name: string): Viewport
    local found = eguidev.viewports({ name = name })[1]
    if found == nil then
        eguidev.wait(function()
            found = eguidev.viewports({ name = name })[1]
            return found ~= nil
        end)
    end
    return assert(found, name .. " viewport should exist")
end

eguidev.fixture("hotki.hud")
eguidev.widget("app.server.connected"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
local hud = require_viewport("hud")
eguidev.widget("hud.panel"):wait({ visible = true })
eguidev.widget("hud.mode"):wait(function(widget)
    return widget ~= nil and widget.value_text == "hud"
end)
eguidev.widget("hud.visible"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
eguidev.widget("hud.geometry.width"):wait(function(widget)
    if widget == nil then
        return false
    end
    local width = tonumber(widget.value_text)
    return width ~= nil and width > 0
end)
eguidev.widget("hud.row.0.desc"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Demo"
end)
assert(assert(eguidev.widget("hud.row.0.chord"):state()).value_text == "d")
assert(assert(eguidev.widget("hud.row.0.is_mode"):state()).value == true)
assert(assert(eguidev.widget("hud.row.0.tag"):state()).value_text ~= "")

eguidev.fixture("hotki.hud.demo")
eguidev.fixture("hotki.hud.mini")
eguidev.widget("app.server.connected"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
hud = require_viewport("hud")
eguidev.widget("hud.mode"):wait(function(widget)
    return widget ~= nil and widget.value_text == "mini"
end)
eguidev.widget("hud.visible"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
eguidev.widget("hud.mini.title"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Mini"
end)
eguidev.widget("hud.geometry.width"):wait(function(widget)
    if widget == nil then
        return false
    end
    local width = tonumber(widget.value_text)
    return width ~= nil and width > 0
end)

eguidev.fixture("hotki.hud.tall")
hud = require_viewport("hud")
eguidev.widget("hud.row.21.desc"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Tall HUD command 21 with enough text to measure"
end)
local panel = eguidev.widget("hud.panel")
local layout_issues = panel:layout_issues()
assert(#layout_issues == 0, "tall HUD panel should have no layout issues")
local geometry_height = tonumber(assert(eguidev.widget("hud.geometry.height"):state()).value_text)
assert(geometry_height ~= nil and geometry_height > 0, "tall HUD should expose a measured height")
for index = 0, 21 do
    local desc = eguidev.widget("hud.row." .. tostring(index) .. ".desc")
    local state = assert(desc:state())
    local measure = desc:text_measure()
    assert(state.visible, desc.id .. " should be visible")
    assert(state.rect.max.y <= (geometry_height :: number) + 1, desc.id .. " should fit in HUD")
    assert(measure.visible_text == state.value_text, desc.id .. " text should be fully visible")
    assert(measure.ellipsis == false, desc.id .. " text should not ellipsize")
end

eguidev.fixture("hotki.hud.pressed")
hud = require_viewport("hud")
eguidev.widget("hud.row.0.pressed"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
assert(assert(eguidev.widget("hud.mode"):state()).value_text == "hud")
assert(assert(eguidev.widget("hud.row.0.stay"):state()).value == true)
assert(assert(eguidev.widget("hud.row.0.chord"):state()).value_text == "p")
assert(assert(eguidev.widget("hud.row.0.desc"):state()).value_text == "Pressed stay command")
layout_issues = eguidev.widget("hud.panel"):layout_issues()
assert(#layout_issues == 0, "pressed HUD panel should have no layout issues")

eguidev.fixture("hotki.hud")
eguidev.fixture("hotki.hud.demo")
eguidev.fixture("hotki.selector")
eguidev.widget("app.server.connected"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
local selector = require_viewport("selector")
eguidev.widget("selector.title"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Pick Demo"
end)
assert(assert(eguidev.widget("selector.placeholder"):state()).value_text == "Search...")
assert(assert(eguidev.widget("selector.selected_index"):state()).value_text == "0")
eguidev.widget("selector.item.0.label"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Alpha"
end)
assert(assert(eguidev.widget("selector.item.0.selected"):state()).value == true)
assert(assert(eguidev.widget("selector.item.0.match_indices"):state()).value_text == "")
eguidev.widget("selector.scroll"):wait(function(widget)
    return widget ~= nil and widget.scroll_state ~= nil
end)

eguidev.fixture("hotki.selector.query")
eguidev.widget("app.server.connected"):wait(function(widget)
    return widget ~= nil and widget.value == true
end)
selector = require_viewport("selector")
eguidev.widget("selector.query"):wait(function(widget)
    return widget ~= nil and string.find(widget.value_text, "b", 1, true) ~= nil
end)
eguidev.widget("selector.item.0.label"):wait(function(widget)
    return widget ~= nil and widget.value_text == "Beta"
end)
assert(assert(eguidev.widget("selector.item.0.selected"):state()).value == true)
assert(assert(eguidev.widget("selector.selected_index"):state()).value_text == "0")
assert(assert(eguidev.widget("selector.item.0.match_indices"):state()).value_text ~= "")

return {
    hud_viewport = hud.id,
    selector_viewport = selector.id,
}
