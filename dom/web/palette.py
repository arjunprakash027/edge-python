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

# Park-Miller LCG; the monotonic counter prevents consecutive calls from clustering visually.
def rand_color() -> str:
    global _rand_seed, _rand_counter
    _rand_counter += 1
    _rand_seed = (_rand_seed * 16807 + _rand_counter * 2654435761) % 2147483647
    return f"#{_rand_seed % 16777216:06x}"

# Builds a swatch under `parent`. Reused for the initial palette and for dynamic adds.
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
    ("Orange", "#d97757"),
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

print("palette ready — click counter, '+ random', or type a hex color + Enter")

async def main():
    n = 0
    while True:
        ev = receive()
        if '"msg":"click"' in ev:
            n += 1
            set_text(btn, f"Clicked {n} time" + ("s" if n != 1 else ""))
            animate(btn, POP, '{"duration": 180}')
        elif '"msg":"add"' in ev:
            mount_swatch(grid, "Random", rand_color())
            animate(last_child(grid), FADE_IN, '{"duration": 240, "fill": "forwards"}')
        elif '"msg":"hex-key"' in ev and '"key":"Enter"' in ev:
            val = get_value(hex_inp)
            if val:
                mount_swatch(grid, "Custom", val)
                animate(last_child(grid), FADE_IN, '{"duration": 240, "fill": "forwards"}')
                set_value(hex_inp, "")
