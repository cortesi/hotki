type Color = string
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

type Offset = {
    x: number,
    y: number,
}

type HudMode = "hud" | "mini" | "hide"
type HudPos = "center" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "nw"
type NotifyPos = "left" | "right"

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

type Style = {
    hud: HudStyle?,
    notify: NotifyStyle?,
    selector: SelectorStyle?,
}
