hotki.root(function(menu, ctx)
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
