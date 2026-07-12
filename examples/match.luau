return function(menu, ctx)
    menu:submenu("shift+cmd+0", "Activate", function(root, inner)
        if inner:app_matches("Safari|Arc|Google Chrome") then
            root:submenu("b", "Browser", function(browser, browser_ctx)
                browser:bind("r", "Reload Tab", function(actx)
                    actx:relay("cmd+r")
                end)
                browser:bind("t", "New Tab", function(actx)
                    actx:relay("cmd+t")
                end)
                browser:bind("p", "Back", function(actx)
                    actx:pop()
                end)
            end)
        end

        if inner:title_matches(".*(?i:readme).*") then
            root:submenu("r", "Reader", function(reader, reader_ctx)
                reader:bind("m", "Mark Read", function(actx)
                    actx:shell("echo marked")
                end)
                reader:bind("p", "Back", function(actx)
                    actx:pop()
                end)
            end)
        end

        root:bind("h", "Hello", function(actx)
            actx:shell("echo hello")
        end)
    end)
end
