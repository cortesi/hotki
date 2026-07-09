type ItemsProvider<T> = (ctx: ModeContext) -> { SelectorItem<T> }
type StringItemsProvider = (ctx: ModeContext) -> { string }

type ModeContext = {
    app: string,
    title: string,
    pid: number,
    hud: boolean,
    depth: number,
    app_matches: (self: ModeContext, pattern: string) -> boolean,
    title_matches: (self: ModeContext, pattern: string) -> boolean,
}

type Action = {
    __kind: "Action",
}

type ShellOptions = {
    ok_notify: NotifyKind?,
    err_notify: NotifyKind?,
}

type RepeatOptions = {
    delay_ms: number?,
    interval_ms: number?,
}

type BindingOptions = {
    hidden: boolean?,
    global: boolean?,
    stay: boolean?,
    ["repeat"]: RepeatOptions?,
}

type SubmenuOptions = {
    hidden: boolean?,
    global: boolean?,
    stay: boolean?,
    ["repeat"]: RepeatOptions?,
    capture: boolean?,
}

type ModeBatchEntry =
    { chord: string, desc: string, action: Action, opts: BindingOptions? }
    | { chord: string, title: string, render: ModeRenderer, opts: SubmenuOptions? }

type ItemSource<T> = { SelectorItem<T> } | { string } | ItemsProvider<T> | StringItemsProvider

type SelectorSpec<T> = {
    title: string?,
    placeholder: string?,
    items: ItemSource<T>,
    on_select: (ctx: ActionContext, item: SelectorItem<T>, query: string) -> (),
    on_cancel: ((ctx: ActionContext) -> ())?,
    max_visible: number?,
}

type MenuBuilder = {
    bind: (
        self: MenuBuilder,
        chord: string,
        desc: string,
        action: Action,
        opts: BindingOptions?
    ) -> (),
    bind_many: (self: MenuBuilder, entries: { ModeBatchEntry }) -> (),
    submenu: (
        self: MenuBuilder,
        chord: string,
        title: string,
        render: ModeRenderer,
        opts: SubmenuOptions?
    ) -> (),
    capture: (self: MenuBuilder) -> (),
}

type ModeRenderer = (menu: MenuBuilder, ctx: ModeContext) -> ()

type ActionContext = {
    app: string,
    title: string,
    pid: number,
    hud: boolean,
    depth: number,
    app_matches: (self: ActionContext, pattern: string) -> boolean,
    title_matches: (self: ActionContext, pattern: string) -> boolean,
    exec: (self: ActionContext, value: Action) -> (),
    notify: (self: ActionContext, kind: NotifyKind, title: string, body: string) -> (),
    stay: (self: ActionContext) -> (),
    push: (self: ActionContext, render: ModeRenderer, title: string?) -> (),
    pop: (self: ActionContext) -> (),
    exit: (self: ActionContext) -> (),
    show_root: (self: ActionContext) -> (),
}

type ActionHandler = (ctx: ActionContext) -> ()

type ActionApi = {
    pop: Action,
    exit: Action,
    show_root: Action,
    hide_hud: Action,
    reload_config: Action,
    clear_notifications: Action,
    shell: (cmd: string, opts: ShellOptions?) -> Action,
    open: (target: string) -> Action,
    relay: (spec: string) -> Action,
    show_details: (toggle: Toggle) -> Action,
    set_volume: (level: number) -> Action,
    change_volume: (delta: number) -> Action,
    mute: (toggle: Toggle) -> Action,
    run: (handler: ActionHandler) -> Action,
    selector: <T>(spec: SelectorSpec<T>) -> Action,
}

type HotkiApi = {
    root: (render: ModeRenderer) -> (),
    applications: ItemsProvider<ApplicationInfo>,
}

declare hotki: {
    root: (render: ModeRenderer) -> (),
    applications: (ctx: ModeContext) -> { SelectorItem<ApplicationInfo> },
}

declare action: {
    pop: Action,
    exit: Action,
    show_root: Action,
    hide_hud: Action,
    reload_config: Action,
    clear_notifications: Action,
    shell: (cmd: string, opts: ShellOptions?) -> Action,
    open: (target: string) -> Action,
    relay: (spec: string) -> Action,
    show_details: (toggle: Toggle) -> Action,
    set_volume: (level: number) -> Action,
    change_volume: (delta: number) -> Action,
    mute: (toggle: Toggle) -> Action,
    run: (handler: ActionHandler) -> Action,
    selector: <T>(spec: SelectorSpec<T>) -> Action,
}
