type Toggle = "on" | "off" | "toggle"
type NotifyKind = "ignore" | "info" | "warn" | "error" | "success"
type HudMode = "hud" | "mini" | "hide"
type HudPos = "center" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "nw"
type NotifyPos = "left" | "right"
type FontWeight =
    "thin"
    | "extralight"
    | "light"
    | "regular"
    | "medium"
    | "semibold"
    | "bold"
    | "extrabold"
    | "black"

type Color = string

type Offset = {
    x: number,
    y: number,
}

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

type ItemsProvider<T> = (ctx: ModeContext) -> { SelectorItem<T> }

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

type BindingStyle = {
    hidden: boolean?,
    key_fg: Color?,
    key_bg: Color?,
    mod_fg: Color?,
    mod_bg: Color?,
    tag_fg: Color?,
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
    style: BindingStyle?,
}

type SubmenuOptions = {
    hidden: boolean?,
    global: boolean?,
    stay: boolean?,
    ["repeat"]: RepeatOptions?,
    style: BindingStyle?,
    capture: boolean?,
}

type HudStyle = {
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

type NotifyWindowStyle = {
    bg: Color?,
    title_fg: Color?,
    body_fg: Color?,
    title_font_size: number?,
    title_font_weight: FontWeight?,
    body_font_size: number?,
    body_font_weight: FontWeight?,
    icon: string?,
}

type NotifyStyle = {
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

type SelectorStyle = {
    bg: Color?,
    input_bg: Color?,
    item_bg: Color?,
    item_selected_bg: Color?,
    match_fg: Color?,
    border: Color?,
    shadow: Color?,
}

type StyleOverlay = {
    hud: HudStyle?,
    notify: NotifyStyle?,
    selector: SelectorStyle?,
}

type ModeBatchEntry =
    { chord: string, desc: string, action: Action, opts: BindingOptions? }
    | { chord: string, title: string, render: ModeRenderer, opts: SubmenuOptions? }

type ItemSource<T> = { SelectorItem<T> } | { string } | ItemsProvider<T>

type SelectorSpec = {
    title: string?,
    placeholder: string?,
    items: any,
    on_select: (ctx: ActionContext, item: SelectorItem<any>, query: string) -> (),
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
    style: (self: MenuBuilder, overlay: StyleOverlay) -> (),
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
type ModeModule = ModeRenderer
type HandlerModule = ActionHandler
type StyleModule = StyleOverlay

type ActionApi = {
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
    selector: (spec: SelectorSpec) -> Action,
}

type ThemesApi = {
    use: (self: ThemesApi, name: string) -> (),
    current: (self: ThemesApi) -> string,
    list: (self: ThemesApi) -> { string },
    get: (self: ThemesApi, name: string) -> StyleOverlay,
    register: (self: ThemesApi, name: string, style: StyleOverlay) -> (),
    remove: (self: ThemesApi, name: string) -> (),
}

type HotkiApi = {
    root: (render: ModeRenderer) -> (),
    applications: ItemsProvider<ApplicationInfo>,
    import_mode: (path: string) -> ModeModule,
    import_items: <T>(path: string) -> ItemsProvider<T>,
    import_handler: (path: string) -> HandlerModule,
    import_style: (path: string) -> StyleModule,
}

declare hotki: {
    root: (render: ModeRenderer) -> (),
    applications: (ctx: ModeContext) -> { SelectorItem<ApplicationInfo> },
    import_mode: (path: string) -> ModeModule,
    import_items: <T>(path: string) -> ItemsProvider<T>,
    import_handler: (path: string) -> HandlerModule,
    import_style: (path: string) -> StyleModule,
}

declare action: {
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
    selector: (spec: SelectorSpec) -> Action,
}

declare themes: {
    use: (self: ThemesApi, name: string) -> (),
    current: (self: ThemesApi) -> string,
    list: (self: ThemesApi) -> { string },
    get: (self: ThemesApi, name: string) -> StyleOverlay,
    register: (self: ThemesApi, name: string, style: StyleOverlay) -> (),
    remove: (self: ThemesApi, name: string) -> (),
}
