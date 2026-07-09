type Toggle = "on" | "off" | "toggle"
type NotifyKind = "ignore" | "info" | "warn" | "error" | "success"

type ApplicationInfo = {
    name: string,
    path: string,
    bundle_id: string?,
}

type SelectorItem<T> = {
    label: string,
    sublabel: string?,
    data: T,
}

type SelectorItemList<T> = { SelectorItem<T> }
type SelectorStringList = { string }
type SelectorItemProvider<T> = (ctx: ModeContext) -> SelectorItemList<T>
type SelectorStringProvider = (ctx: ModeContext) -> SelectorStringList

type ModeContext = {
    app: string,
    title: string,
    pid: number,
    hud: boolean,
    depth: number,
    app_matches: (self: ModeContext, pattern: string) -> boolean,
    title_matches: (self: ModeContext, pattern: string) -> boolean,
}

type Action = (ctx: ActionContext) -> ()

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
}

type SubmenuOptions = {
    hidden: boolean?,
    global: boolean?,
    stay: boolean?,
    capture: boolean?,
}

type ItemSource<T> =
    SelectorItemList<T> | SelectorStringList | SelectorItemProvider<T> | SelectorStringProvider

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
    notify: (self: ActionContext, kind: NotifyKind, title: string, body: string) -> (),
    stay: (self: ActionContext) -> (),
    push: (self: ActionContext, render: ModeRenderer, title: string?) -> (),
    pop: (self: ActionContext) -> (),
    exit: (self: ActionContext) -> (),
    show_root: (self: ActionContext) -> (),
    hide_hud: (self: ActionContext) -> (),
    reload_config: (self: ActionContext) -> (),
    clear_notifications: (self: ActionContext) -> (),
    shell: (self: ActionContext, cmd: string, opts: ShellOptions?) -> (),
    open: (self: ActionContext, target: string) -> (),
    relay: (self: ActionContext, spec: string) -> (),
    show_details: (self: ActionContext, toggle: Toggle) -> (),
    set_volume: (self: ActionContext, level: number) -> (),
    change_volume: (self: ActionContext, delta: number) -> (),
    mute: (self: ActionContext, toggle: Toggle) -> (),
    until_keyup: (self: ActionContext, action: Action, opts: RepeatOptions?) -> (),
    select: <T>(self: ActionContext, spec: SelectorSpec<T>) -> (),
}

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
    selector: <T>(spec: SelectorSpec<T>) -> Action,
}

type HotkiApi = {
    root: (render: ModeRenderer) -> (),
    applications: SelectorItemProvider<ApplicationInfo>,
}

declare hotki: HotkiApi

declare action: ActionApi
