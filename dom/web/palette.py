STYLES: dict[str, str] = {
    "box": "aspect-square rounded-[10px] p-3 flex items-end shadow-sm",
    "label": "text-white font-mono text-xs mix-blend-difference",
    "btn": "bg-neutral-900 text-white px-4 py-2 rounded text-sm hover:bg-neutral-700 transition mb-6",
}

from dom import (
    query, create_element, append_child, insert_before,
    set_text, set_attribute, bind_event,
)

class Swatch:
    def __init__(self, name: str, color: str):
        self.name = name
        self.color = color

    def mount(self, parent):
        box = create_element("div")
        set_attribute(box, "class", STYLES["box"])
        set_attribute(box, "style", f"background:{self.color}")

        label = create_element("span")
        set_attribute(label, "class", STYLES["label"])
        set_text(label, f"{self.name} {self.color}")

        append_child(box, label)
        append_child(parent, box)

PALETTE: list[Swatch] = [
    Swatch("Claude", "#d97757"),
    Swatch("Slate", "#1f2937"),
    Swatch("Sky", "#0ea5e9"),
    Swatch("Lime", "#84cc16"),
    Swatch("Rose", "#f43f5e"),
    Swatch("Amber", "#f59e0b"),
]

grid = query("#palette")
for swatch in PALETTE:
    swatch.mount(grid)

btn = create_element("button")
set_attribute(btn, "class", STYLES["btn"])
set_text(btn, "Clicked 0 times")
bind_event(btn, "click", "click")
insert_before(btn, grid)

async def main():
    n = 0
    while True:
        receive()
        n += 1
        set_text(btn, f"Clicked {n} time" + ("s" if n != 1 else ""))
