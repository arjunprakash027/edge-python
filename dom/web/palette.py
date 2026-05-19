from dom import (
    query, create_element, append_child, last_child,
    set_text, set_value, get_value, set_attribute, bind_event,
    animate,
)

STYLES: dict[str, str] = {
    "box": "aspect-square rounded-[10px] p-3 flex items-end shadow-sm",
    "label": "text-white font-mono text-xs mix-blend-difference",
}

POP = '[{"transform": "scale(1)"}, {"transform": "scale(1.08)"}, {"transform": "scale(1)"}]'
FADE_IN = '[{"opacity": 0, "transform": "scale(0.7)"}, {"opacity": 1, "transform": "scale(1)"}]'

_rand_seed = 42
_rand_counter = 0

# Park-Miller LCG mixed with a monotonic counter so consecutive calls don't visually cluster.
def rand_color() -> str:
    global _rand_seed, _rand_counter
    _rand_counter += 1
    _rand_seed = (_rand_seed * 16807 + _rand_counter * 2654435761) % 2147483647
    return f"#{_rand_seed % 16777216:06x}"

# Free function instead of method to dodge an upstream bug: methods on instances
# constructed inside a resumed coroutine silently kill the coroutine.
def mount_swatch(parent, name: str, color: str):
    box = create_element("div")
    set_attribute(box, "class", STYLES["box"])
    set_attribute(box, "style", f"background:{color}")

    label = create_element("span")
    set_attribute(label, "class", STYLES["label"])
    set_text(label, f"{name} {color}")

    append_child(box, label)
    append_child(parent, box)

PALETTE: list[tuple[str, str]] = [
    ("Claude", "#d97757"),
    ("Slate", "#1f2937"),
    ("Sky", "#0ea5e9"),
    ("Lime", "#84cc16"),
    ("Rose", "#f43f5e"),
    ("Amber", "#f59e0b"),
]

grid = query("#palette")
for name, color in PALETTE:
    mount_swatch(grid, name, color)

btn = query("#counter")
add_btn = query("#add-random")
hex_inp = query("#hex-input")
bind_event(btn, "click", "click")
bind_event(add_btn, "click", "add")
bind_event(hex_inp, "keydown", "hex-key")

async def main():
    n = 0
    it = 0
    while True:
        ev = receive()
        it += 1
        print(f"tick {it}: {ev[:30]}")
        if '"msg":"click"' in ev:
            n += 1
            print(f"  click: n={n}")
            set_text(btn, f"Clicked {n} time" + ("s" if n != 1 else ""))
            print("  click: set_text ok")
            animate(btn, POP, '{"duration": 180}')
            print("  click: animate ok")
        elif '"msg":"add"' in ev:
            print("  add: branch entered")
            c = rand_color()
            print(f"  add: color={c}")
            s = Swatch("Random", c)
            print("  add: swatch constructed")
            s.mount(grid)
            print("  add: mounted")
            h = last_child(grid)
            print(f"  add: last_child={h}")
            animate(h, FADE_IN, '{"duration": 240, "fill": "forwards"}')
            print("  add: animated")
        elif '"msg":"hex-key"' in ev and '"key":"Enter"' in ev:
            val = get_value(hex_inp)
            if val:
                Swatch("Custom", val).mount(grid)
                animate(last_child(grid), FADE_IN, '{"duration": 240, "fill": "forwards"}')
                set_value(hex_inp, "")
