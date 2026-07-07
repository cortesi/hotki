type Toggle = "on" | "off" | "toggle"
type NotifyKind = "ignore" | "info" | "warn" | "error" | "success"
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
