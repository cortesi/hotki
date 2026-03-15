themes:use("default")

hotki.root(function(menu, ctx)
    menu:style({
        hud = {
            pos = "ne",
            font_size = 16,
            key_font_size = 16,
            title_font_weight = "regular",
            key_font_weight = "regular",
            tag_font_size = 16,
            tag_font_weight = "regular",
            title_fg = "white",
            key_fg = "white",
            key_bg = "#303030",
            mod_fg = "white",
            mod_bg = "#404040",
            tag_fg = "white",
            bg = "#202020",
            opacity = 0.95,
        },
    })

    menu:submenu("shift+cmd+0", "Activate", function(root, inner)
        if inner:app_matches("Safari|Arc|Google Chrome") then
            root:submenu("b", "Browser", function(browser, browser_ctx)
                browser:bind("r", "Reload Tab", action.relay("cmd+r"))
                browser:bind("t", "New Tab", action.relay("cmd+t"))
                browser:bind("p", "Back", action.pop)
            end)
        end

        if inner:title_matches(".*(?i:readme).*") then
            root:submenu("r", "Reader", function(reader, reader_ctx)
                reader:bind("m", "Mark Read", action.shell("echo marked"))
                reader:bind("p", "Back", action.pop)
            end)
        end

        root:bind("h", "Hello", action.shell("echo hello"))
    end)
end)
