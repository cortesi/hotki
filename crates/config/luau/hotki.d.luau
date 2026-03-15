export type Toggle = "on" | "off" | "toggle"
export type NotifyKind = "ignore" | "info" | "warn" | "error" | "success"
export type HudMode = "hud" | "mini" | "hide"
export type HudPos = "center" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "nw"
export type NotifyPos = "left" | "right"
export type FontWeight =
    "thin"
    | "extralight"
    | "light"
    | "regular"
    | "medium"
    | "semibold"
    | "bold"
    | "extrabold"
    | "black"

export type Color = string

export type Offset = {
    x: number,
    y: number,
}

export type ApplicationInfo = {
    name: string,
    path: string,
    bundle_id: string?,
}

export type SelectorItem<T> = {
    label: string,
    sublabel: string?,
    data: T,
}

export type ItemsProvider<T> = (ctx: ModeContext) -> { SelectorItem<T> }

export type ModeContext = {
    app: string,
    title: string,
    pid: number,
    hud: boolean,
    depth: number,
    app_matches: (self: ModeContext, pattern: string) -> boolean,
    title_matches: (self: ModeContext, pattern: string) -> boolean,
}

export type ActionContext = ModeContext & {
    exec: (self: ActionContext, value: Action) -> (),
    notify: (self: ActionContext, kind: NotifyKind, title: string, body: string) -> (),
    stay: (self: ActionContext) -> (),
    push: (self: ActionContext, render: ModeRenderer, title: string?) -> (),
    pop: (self: ActionContext) -> (),
    exit: (self: ActionContext) -> (),
    show_root: (self: ActionContext) -> (),
}

export type Action = {
    __kind: "Action",
}

export type ShellOptions = {
    ok_notify: NotifyKind?,
    err_notify: NotifyKind?,
}

export type BindingStyle = {
    hidden: boolean?,
    key_fg: Color?,
    key_bg: Color?,
    mod_fg: Color?,
    mod_bg: Color?,
    tag_fg: Color?,
}

export type RepeatOptions = {
    delay_ms: number?,
    interval_ms: number?,
}

export type BindingOptions = {
    hidden: boolean?,
    global: boolean?,
    stay: boolean?,
    ["repeat"]: RepeatOptions?,
    style: BindingStyle?,
}

export type SubmenuOptions = {
    hidden: boolean?,
    global: boolean?,
    stay: boolean?,
    ["repeat"]: RepeatOptions?,
    style: BindingStyle?,
    capture: boolean?,
}

export type HudStyle = {
    mode: HudMode?,
    pos: HudPos?,
    offset: Offset?,
    font_size: number?,
    title_font_weight: FontWeight?,
    key_font_size: number?,
    key_font_weight: FontWeight?,
    tag_font_size: number?,
    tag_font_weight: FontWeight?,
    title_fg: Color?,
    bg: Color?,
    key_fg: Color?,
    key_bg: Color?,
    mod_fg: Color?,
    mod_font_weight: FontWeight?,
    mod_bg: Color?,
    tag_fg: Color?,
    opacity: number?,
    key_radius: number?,
    key_pad_x: number?,
    key_pad_y: number?,
    radius: number?,
    tag_submenu: string?,
}

export type NotifyWindowStyle = {
    bg: Color?,
    title_fg: Color?,
    body_fg: Color?,
    title_font_size: number?,
    title_font_weight: FontWeight?,
    body_font_size: number?,
    body_font_weight: FontWeight?,
    icon: string?,
}

export type NotifyStyle = {
    width: number?,
    pos: NotifyPos?,
    opacity: number?,
    timeout: number?,
    buffer: number?,
    radius: number?,
    info: NotifyWindowStyle?,
    warn: NotifyWindowStyle?,
    error: NotifyWindowStyle?,
    success: NotifyWindowStyle?,
}

export type SelectorStyle = {
    bg: Color?,
    input_bg: Color?,
    item_bg: Color?,
    item_selected_bg: Color?,
    match_fg: Color?,
    border: Color?,
    shadow: Color?,
}

export type StyleOverlay = {
    hud: HudStyle?,
    notify: NotifyStyle?,
    selector: SelectorStyle?,
}

export type ModeBatchEntry =
    { chord: string, desc: string, action: Action, opts: BindingOptions? }
    | { chord: string, title: string, render: ModeRenderer, opts: SubmenuOptions? }

export type ItemSource<T> = { SelectorItem<T> } | ItemsProvider<T>

export type SelectorSpec<T> = {
    title: string?,
    placeholder: string?,
    items: any,
    on_select: (ctx: ActionContext, item: any, query: string) -> (),
    on_cancel: ((ctx: ActionContext) -> ())?,
    max_visible: number?,
}

export type MenuBuilder = {
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
    style: (self: MenuBuilder, overlay: StyleOverlay) -> (),
    capture: (self: MenuBuilder) -> (),
}

export type ModeRenderer = (menu: MenuBuilder, ctx: ModeContext) -> ()
export type ActionHandler = (ctx: ActionContext) -> ()
export type ModeModule = ModeRenderer
export type HandlerModule = ActionHandler
export type StyleModule = StyleOverlay

export type ActionApi = {
    pop: Action,
    exit: Action,
    show_root: Action,
    hide_hud: Action,
    reload_config: Action,
    clear_notifications: Action,
    theme_next: Action,
    theme_prev: Action,
    shell: (cmd: string, opts: ShellOptions?) -> Action,
    open: (target: string) -> Action,
    relay: (spec: string) -> Action,
    show_details: (toggle: Toggle) -> Action,
    theme_set: (name: string) -> Action,
    set_volume: (level: number) -> Action,
    change_volume: (delta: number) -> Action,
    mute: (toggle: Toggle) -> Action,
    run: (handler: ActionHandler) -> Action,
    selector: (spec: SelectorSpec<any>) -> Action,
}

export type ThemesApi = {
    use: (self: ThemesApi, name: string) -> (),
    current: (self: ThemesApi) -> string,
    list: (self: ThemesApi) -> { string },
    get: (self: ThemesApi, name: string) -> StyleOverlay,
    register: (self: ThemesApi, name: string, style: StyleOverlay) -> (),
    remove: (self: ThemesApi, name: string) -> (),
}

export type HotkiApi = {
    root: (render: ModeRenderer) -> (),
    applications: ItemsProvider<ApplicationInfo>,
    import_mode: (path: string) -> ModeModule,
    import_items: <T>(path: string) -> ItemsProvider<T>,
    import_handler: (path: string) -> HandlerModule,
    import_style: (path: string) -> StyleModule,
}

declare hotki: HotkiApi
declare action: ActionApi
declare themes: ThemesApi
