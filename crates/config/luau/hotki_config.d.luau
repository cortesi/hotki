type Toggle = "on" | "off" | "toggle"
type NotifyKind = "ignore" | "info" | "warn" | "error" | "success"

type ApplicationInfo = {
    read name: string,
    read path: string,
    read bundle_id: string?,
}

type SelectorItem<T> = {
    read label: string,
    read sublabel: string?,
    read data: T,
}

type SelectorItemList<T> = { read [number]: SelectorItem<T> }
type SelectorStringList = { read [number]: string }
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
    read ok_notify: NotifyKind?,
    read err_notify: NotifyKind?,
}

type RepeatOptions = {
    read delay_ms: number?,
    read interval_ms: number?,
}

type BindingOptions = {
    read hidden: boolean?,
    read global: boolean?,
    read stay: boolean?,
}

type SubmenuOptions = {
    read hidden: boolean?,
    read global: boolean?,
    read stay: boolean?,
    read capture: boolean?,
}

type ItemSource<T> =
    SelectorItemList<T> | SelectorStringList | SelectorItemProvider<T> | SelectorStringProvider

type SelectorSpec<T> = {
    read title: string?,
    read placeholder: string?,
    read items: ItemSource<T>,
    read on_select: (ctx: ActionContext, item: SelectorItem<T>, query: string) -> (),
    read on_cancel: ((ctx: ActionContext) -> ())?,
    read max_visible: number?,
}

type LaunchApplicationOptions = {
    read title: string?,
    read placeholder: string?,
    read max_visible: number?,
}

type ExecSpec = {
    read program: string,
    read args: { read [number]: string }?,
    read cwd: string?,
    read ok_notify: NotifyKind?,
    read err_notify: NotifyKind?,
}

type MenuBuilder = {
    with: (self: MenuBuilder, defaults: BindingOptions) -> MenuBuilder,
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

type Renderers = {
    combine: (...ModeRenderer) -> ModeRenderer,
    when_app: (app: string, renderer: ModeRenderer) -> ModeRenderer,
    when_app_matches: (pattern: string, renderer: ModeRenderer) -> ModeRenderer,
}

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
    exec: (self: ActionContext, spec: ExecSpec) -> (),
    open: (self: ActionContext, target: string) -> (),
    relay: (self: ActionContext, spec: string) -> (),
    relay_to_app: (self: ActionContext, app_name: string, spec: string) -> (),
    show_details: (self: ActionContext, toggle: Toggle) -> (),
    set_volume: (self: ActionContext, level: number) -> (),
    change_volume: (self: ActionContext, delta: number) -> (),
    mute: (self: ActionContext, toggle: Toggle) -> (),
    until_keyup: (self: ActionContext, action: Action, opts: RepeatOptions?) -> (),
    select: <T>(self: ActionContext, spec: SelectorSpec<T>) -> (),
}

type Actions = {
    pop: Action,
    exit: Action,
    show_root: Action,
    hide_hud: Action,
    reload_config: Action,
    clear_notifications: Action,
    stay: Action,
    notify: (kind: NotifyKind, title: string, body: string) -> Action,
    push: (render: ModeRenderer, title: string?) -> Action,
    shell: (cmd: string, opts: ShellOptions?) -> Action,
    exec: (spec: ExecSpec) -> Action,
    open: (target: string) -> Action,
    relay: (spec: string) -> Action,
    relay_to_app: (app_name: string) -> (spec: string) -> Action,
    relay_with: (prefix: string) -> (spec: string) -> Action,
    launch_application: (options: LaunchApplicationOptions?) -> Action,
    show_details: (toggle: Toggle) -> Action,
    set_volume: (level: number) -> Action,
    change_volume: (delta: number) -> Action,
    mute: (toggle: Toggle) -> Action,
    hold: (action: Action, opts: RepeatOptions?) -> Action,
    select: <T>(spec: SelectorSpec<T>) -> Action,
}

declare hotki: {
    actions: Actions,
    renderers: Renderers,
    applications: (ctx: ModeContext) -> SelectorItemList<ApplicationInfo>,
}
